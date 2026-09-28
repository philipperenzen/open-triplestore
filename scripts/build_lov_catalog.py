#!/usr/bin/env python3
"""Build the vendored LOV vocabulary catalog from a lov.nq.gz dump.

The Linked Open Vocabularies (LOV) N-Quads dump contains one named graph per
vocabulary (latest version) plus the LOV metadata graph
<https://lov.linkeddata.es/dataset/lov> describing every vocabulary (titles,
descriptions, tags, namespaces, prefixes, versions, VOAF relations and reuse
metrics).  This script distils that metadata graph into the compact JSON
catalog the backend embeds (assets/vocab/lov-catalog.json.gz), so vocabulary
search works without the full 18 MB corpus at hand.

Licences.  LOV publishes its own data under CC BY 4.0, but that only covers
what LOV holds rights in.  Each vocabulary graph is the publisher's work under
the publisher's terms, and many of LOV's descriptions are copied from those
graphs.  So the script also reads the licence each vocabulary declares in its
OWN graph (the metadata graph carries none per vocabulary) and classifies it:

  * redistributable: the licence lets anyone share an unmodified copy with
    attribution and does not restrict use (CC0 / public-domain tools, CC BY,
    BY-SA and BY-ND of any version, ODC, Apache-2.0, MIT, BSD, ISC, W3C, OGC
    and attribution-only open government licences).  What ships is LOV's
    N-Quads serialization of each graph as LOV's dump holds it: the
    publisher's triples in another format, which even the licences that
    forbid modification allow ("copy and distribute the contents ... in any
    medium").  Where LOV mis-decoded characters (UTF-8 read as Latin-1 or
    Windows-1252), its copy differs from the publisher's document in those
    literals: such a graph ships only under a licence that allows
    modification, and its notice says so (lov_misdecoded);
  * not redistributable: NonCommercial, all rights reserved or only a
    copyright line, copyleft that would oblige us to ship its text (GPL,
    LGPL, AGPL, GFDL, EUPL, MPL), anything unrecognised, and no licence at all.

Some publishers state their terms outside the graph instead: W3C's Document
License for the documents on its site, DCMI's CC BY 4.0 for its schemas, the
FOAF and schema.org specifications, ESCO's reuse terms and so on.  When a
graph names no licence (none, or only a copyright line), the script applies
those terms from a reviewed table (PUBLISHER_TERMS), each row backed by a
primary source from the rights holder, and records that in license_source
and license_source_url.  A licence the graph itself states is never
overridden.

Notices.  Where a licence requires a statement on copies, license_notice
carries it, whatever the licence's source: the W3C and OGC document notices
(built per graph from the graph's own title, date and status; see
w3c_document_notice), the copyright lines MIT and BSD require (GRAPH_NOTICES,
each quoted from the rights holder's own licence file), DCMI's and ESCO's
statements, and so on.  The build fails if a W3C notice would carry an
unfilled placeholder.

A vocabulary whose licence would allow redistribution is still withheld
(redistribution_withheld says why) when LOV's copy is not a faithful copy of a
work that allows no modification (WITHHELD, and a check for text LOV
mis-decoded), when its licence requires a copyright notice that the rights
holder does not publish, or when the licence is not identified precisely
enough to meet its conditions (WITHHELD).

Each licence label gets its URI (license_uris, see LICENSE_URIS), and
no_derivatives marks the vocabularies whose every licence allows only
unaltered copies (CC BY-ND, the OGC Document Notice): an installed copy of
one may not be edited.

Every vocabulary keeps its facts (prefix, namespace, dates, versions, LOV's
tags and reuse metrics, titles, creator and publisher names).  Its description
is kept only when the vocabulary is redistributable — and, for licences that
forbid modification, only when LOV's text is character for character a
literal in the vocabulary's own graph.

A second output, the allowlist (--allowlist, normally
assets/vocab/lov-redistributable.txt), lists the graphs of the redistributable
vocabularies with their licence and required notice.  The Docker image filters
the corpus it ships down to exactly those graphs.

Usage:
    python scripts/build_lov_catalog.py <lov.nq.gz> <out-catalog.json.gz> \
        [--allowlist assets/vocab/lov-redistributable.txt] \
        [--snapshot-date YYYY-MM-DD] [--source-url URL]
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

# Greedy object: the graph is the LAST <...> term, even when a literal
# contains " <" (a lazy match would cut the literal there).
LINE_RE = re.compile(r"^(\S+)\s+(<[^>]+>)\s+(.*)\s+(<[^>]+>)\s*\.\s*$")
LIT_RE = re.compile(r'^"(.*)"(?:@([a-zA-Z-]+)|\^\^<[^>]+>)?$', re.DOTALL)

VOAF = "http://purl.org/vocommons/voaf#"
DCT = "http://purl.org/dc/terms/"
DC = "http://purl.org/dc/elements/1.1/"
VANN = "http://purl.org/vocab/vann/"
DCAT = "http://www.w3.org/ns/dcat#"
FOAF = "http://xmlns.com/foaf/0.1/"
RDF_TYPE = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>"

LOV_LICENSE = "CC BY 4.0"
LOV_LICENSE_URL = "https://creativecommons.org/licenses/by/4.0/"

# Relations whose presence counts toward a vocabulary's incoming links.
VOAF_RELATIONS = [
    "metadataVoc",
    "specializes",
    "generalizes",
    "extends",
    "hasEquivalencesWith",
    "hasDisjunctionsWith",
]

# Predicates a vocabulary uses to state its licence or rights, including the
# misspellings and legacy namespaces that occur in the wild.
LICENSE_PREDICATES = {
    DCT + "license",
    DCT + "licence",
    DCT + "rights",
    DC + "license",
    DC + "rights",
    "http://creativecommons.org/ns#license",
    "http://creativecommons.org/ns#licence",
    "https://creativecommons.org/ns#license",
    "https://creativecommons.org/ns#licence",
    "http://web.resource.org/cc/license",
    "http://www.w3.org/1999/xhtml/vocab#license",
    "http://schema.org/license",
    "https://schema.org/license",
    # The SWAP files' own licence pointer: doc and gso state
    # doc:ipr <http://www.w3.org/2000/10/swap/LICENSE.n3>.
    "http://www.w3.org/2000/10/swap/pim/doc#ipr",
}

OWL = "http://www.w3.org/2002/07/owl#"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"

# What the notices need from a vocabulary's own graph (see
# w3c_document_notice): its title, its dates and any status it states.
NODE_TITLE = [DCT + "title", DC + "title", RDFS + "label"]
NODE_DATES = [
    DCT + "modified",
    DC + "modified",
    DCT + "date",
    DC + "date",
    DCT + "issued",
    DC + "issued",
]
NODE_CREATED = [DCT + "created", DC + "created"]
# earl's graph misspells owl:versionInfo; the statement is still its own.
NODE_VERSION_INFO = [OWL + "versionInfo", "http://www.w3.org/2002/07/owlversionInfo"]
NODE_VERSION_IRI = [OWL + "versionIRI"]
NODE_STATUS = ["http://purl.org/ontology/bibo/status"]
NODE_META_PREDICATES = set(
    NODE_TITLE + NODE_DATES + NODE_CREATED + NODE_VERSION_INFO + NODE_VERSION_IRI + NODE_STATUS
)

# Types marking the node that describes the vocabulary itself.
VOCAB_NODE_TYPES = {
    "http://www.w3.org/2002/07/owl#Ontology",
    VOAF + "Vocabulary",
    "http://www.w3.org/2004/02/skos/core#ConceptScheme",
}


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


def norm_ws(s: str) -> str:
    return " ".join(s.split())


# ─── Licence classification ──────────────────────────────────────────────────
#
# Each declared value classifies to zero or more (label, class) pairs:
#   open        a licence that allows redistributing copies with attribution
#   noderiv     like open, but modification is not allowed: copies of the
#               contents only (LOV's copy must then be faithful, see WITHHELD)
#   restricted  a recognised licence or statement that does not qualify
#   unknown     a licence reference this script cannot identify
#   notice      a copyright line or rights-holder name with no licence terms

OPEN, NODERIV, RESTRICTED, UNKNOWN, NOTICE = "open", "noderiv", "restricted", "unknown", "notice"

# Licence documents that are not a standard licence URL, checked by hand on
# 2026-09-23 against the document itself.
VERIFIED_DOCUMENTS = {
    # MIT (Expat) text, "Copyright (C) 2019 Huanyu Li".
    "github.com/huanyu-li/materials-design-ontology/blob/master/license": ("MIT", OPEN),
    # "the content of the repositories in the ETSI Forge is licensed under the
    # terms of the BSD-3-Clause LICENSE".
    "forge.etsi.org/etsi-software-license": ("BSD-3-Clause", OPEN),
    # Flemish government reuse licence: attribution only, commercial reuse
    # allowed (archived copy of 2023-05-24; the live page is gone).
    "overheid.vlaanderen.be/sites/default/files/documenten/ict-egov/licenties/hergebruik/modellicentie_gratis_hergebruik_v1_0.html": (
        "Modellicentie Gratis Hergebruik 1.0",
        OPEN,
    ),
    # EU ISA programme: use and re-use for any purpose, keeping the copyright
    # notice and the licence's no-warranty disclaimer (archived 2017-07-16).
    "joinup.ec.europa.eu/category/licence/isa-open-metadata-licence-v11": (
        "ISA Open Metadata Licence 1.1",
        OPEN,
    ),
    # OGC Document License Agreement: use, copy, distribute, keeping notices.
    "ogc.org/license": ("OGC Document License", OPEN),
    # Now lists OGC Software License 1.0 and Apache-2.0.
    "opengeospatial.org/ogc/software": ("OGC Software License", OPEN),
    # W3C's intellectual-rights notice of 2002: an index of W3C's policies
    # and licences, not a licence.  Only ODRL points to it; the terms of its
    # specification apply (see the odrl row of PUBLISHER_TERMS).
    "w3.org/consortium/legal/2002/ipr-notice-20021231": ("W3C intellectual-rights notice", NOTICE),
    # The SWAP distribution's licence file, which doc and gso name with
    # doc:ipr: "This licence is used for files in this distribution",
    # doc:subLicense <http://www.w3.org/Consortium/Legal/copyright-software-19980720>,
    # comment "Share and Enjoy. Open Source license: Copyright (c) 2000,1,2
    # W3C (MIT, INRIA, Keio) ...".  That is the W3C Software Notice and
    # License of 1998: use, copy, modify and distribute, keeping its full
    # text, the pre-existing notices and a notice of changes.
    "w3.org/2000/10/swap/license.n3": ("W3C Software Notice (1998)", OPEN),
    "w3.org/consortium/legal/copyright-software-19980720": ("W3C Software Notice (1998)", OPEN),
    # OGC Document Notice: "Public documents on the OGC site are provided by
    # the copyright holders under the following license" — copy and
    # distribute with the notices; "No right to create modifications or
    # derivatives of OGC documents is granted".
    "ogc.org/about/policies/document-notice": ("OGC Document Notice", NODERIV),
    "ogc.org/about-ogc/policies/document-notice": ("OGC Document Notice", NODERIV),
}

COPYLEFT = [
    (re.compile(r"\bagpl|affero"), "AGPL"),
    (re.compile(r"\blgpl|lesser general public|library general public"), "LGPL"),
    (re.compile(r"\bgfdl\b|free documentation licen|copyleft/fdl|licenses/fdl"), "GFDL"),
    (re.compile(r"\bgpl|gnu general public|licenses/gpl"), "GPL"),
    (re.compile(r"\beupl\b|dec_impl/2017/863"), "EUPL"),
    (re.compile(r"\bmpl\b|mozilla public licen|mozilla\.org/mpl"), "MPL"),
]


def cc_label(parts: list[str], version: str | None, port: str | None = None) -> tuple[str, str]:
    """Label and class for a Creative Commons licence given its elements."""
    code = "-".join(p.upper() for p in parts)
    label = f"CC {code}" + (f" {version}" if version else "") + (f" {port.upper()}" if port else "")
    if parts[0] != "by":
        return label, UNKNOWN
    if "nc" in parts:
        return label, RESTRICTED
    if "nd" in parts:
        return label, NODERIV
    return label, OPEN


def classify_iri(iri: str) -> tuple[str, str] | None:
    """Classify a licence IRI; None when it is not recognisably a licence."""
    u = iri.strip().strip("<>").strip().lower()
    u = re.sub(r"^https?://", "", u)
    u = re.sub(r"^www\.", "", u)
    u = u.rstrip("/")
    for doc, result in VERIFIED_DOCUMENTS.items():
        if u == doc or u.startswith(doc + "#") or u.startswith(doc + "/"):
            return result

    m = re.match(r"creativecommons\.org/publicdomain/(zero|mark)/(\d\.\d)", u)
    if m:
        return ("CC0 " if m.group(1) == "zero" else "Public Domain Mark ") + m.group(2), OPEN
    if re.match(r"creativecommons\.org/licen[cs]es?/publicdomain", u):
        return "CC Public Domain", OPEN
    m = re.match(r"creativecommons\.org/licen[cs]es?/([a-z-]+)(?:/(\d(?:\.\d)?))?(?:/([a-z]{2})(?=/|$|#))?", u)
    if m:
        version = m.group(2)
        if version and "." not in version:
            version += ".0"
        return cc_label(m.group(1).split("-"), version, m.group(3))

    # purl.org/NET/rdflicense ids (cc-by4.0, APACHE2.0, MIT1.0, ...).
    m = re.match(r"purl\.org/net/rdflicense/(.+)", u)
    if m:
        rid = m.group(1)
        mm = re.match(r"cc-(by(?:-nc)?(?:-nd|-sa)?)(\d\.\d)", rid)
        if mm:
            return cc_label(mm.group(1).split("-"), mm.group(2))
        if rid.startswith("apache2"):
            return "Apache-2.0", OPEN
        if rid.startswith("mit"):
            return "MIT", OPEN
        return None

    m = re.match(r"spdx\.org/licenses/([a-z0-9.+-]+?)(?:\.html|\.json)?$", u)
    if m:
        return classify_spdx(m.group(1))

    m = re.match(r"opendatacommons\.org/licenses/(by|odbl|pddl)(?:/(\d[.-]\d))?", u)
    if m:
        name = {"by": "ODC-By", "odbl": "ODbL", "pddl": "PDDL"}[m.group(1)]
        # Each of the three has only ever had version 1.0.
        return f"{name} {(m.group(2) or '1.0').replace('-', '.')}", OPEN
    if re.match(r"apache\.org/licenses/license-2\.0", u) or u == "opensource.org/licenses/apache-2.0":
        return "Apache-2.0", OPEN
    m = re.match(r"(?:opensource\.org/licenses|dalicc\.net/licenselibrary)/([a-z0-9.-]+)", u)
    if m:
        return classify_spdx(m.group(1))
    if u.startswith("unlicense.org"):
        return "Unlicense", OPEN

    m = re.match(r"w3\.org/consortium/legal/(\d{4})/(copyright-software-and-document|copyright-software|doc-license|copyright-documents)", u)
    if m:
        kind = m.group(2)
        if kind == "copyright-software-and-document":
            return f"W3C Software and Document License ({m.group(1)})", OPEN
        if kind == "copyright-software":
            return f"W3C Software License ({m.group(1)})", OPEN
        return f"W3C Document License ({m.group(1)})", NODERIV
    m = re.match(r"w3\.org/copyright/(software|document)-license-(\d{4})", u)
    if m:
        if m.group(1) == "software":
            return f"W3C Software License ({m.group(2)})", OPEN
        return f"W3C Document License ({m.group(2)})", NODERIV

    if "licence-ouverte" in u or ("etalab" in u and "open-licence" in u):
        return "Licence Ouverte (Etalab)", OPEN
    if u.startswith("nationalarchives.gov.uk/doc/open-government-licence"):
        return "UK Open Government Licence", OPEN
    m = re.match(r"formez\.it/iodl/(\d\.\d)", u)
    if m and m.group(1) == "2.0":
        return "IODL 2.0", OPEN
    # Italian public-sector licence code list, e.g. .../licences/A31_CCBYSA40.
    m = re.search(r"controlled-vocabulary/licences/a\d+_cc(by)(nc)?(nd|sa)?(\d)(\d)$", u)
    if m:
        parts = [p for p in m.groups()[:3] if p]
        return cc_label(parts, f"{m.group(4)}.{m.group(5)}")

    for pattern, name in COPYLEFT:
        if pattern.search(u):
            return name, RESTRICTED
    if "joinup.ec.europa.eu" in u and "eupl" in u:
        return "EUPL", RESTRICTED

    # Licence IRIs minted by other vocabularies (oegov.org's
    # ...#CreativeCommonsAttributionShareAlike3.0_UnitedStatesLicense): read
    # the words of the last segment.
    tail = re.split(r"[#/]", iri.strip().strip("<>").rstrip("/#"))[-1]
    words = re.sub(r"([a-z])([A-Z])", r"\1 \2", tail).replace("_", " ")
    # "ShareAlike3.0" -> "ShareAlike 3.0", so the version survives.
    words = re.sub(r"([A-Za-z])(\d)", r"\1 \2", words)
    if "creative commons" in words.lower():
        return classify_words(words.lower())
    return None


def classify_spdx(sid: str) -> tuple[str, str]:
    s = sid.lower()
    m = re.match(r"cc-(by(?:-nc)?(?:-nd|-sa)?)-(\d\.\d)", s)
    if m:
        return cc_label(m.group(1).split("-"), m.group(2))
    if s.startswith("cc0"):
        return "CC0 1.0", OPEN
    table = {
        "mit": "MIT",
        "apache-2.0": "Apache-2.0",
        "bsd-2-clause": "BSD-2-Clause",
        "bsd-3-clause": "BSD-3-Clause",
        "isc": "ISC",
        "odbl-1.0": "ODbL 1.0",
        "pddl-1.0": "PDDL 1.0",
        "odc-by-1.0": "ODC-By 1.0",
        "w3c": "W3C Software License",
        "w3c-20150513": "W3C Software and Document License (2015)",
        "unlicense": "Unlicense",
    }
    if s in table:
        return table[s], OPEN
    for pattern, name in COPYLEFT:
        if pattern.search(s):
            return name, RESTRICTED
    if s.startswith("bsd-4"):
        # The advertising clause binds everyone downstream.
        return "BSD-4-Clause", RESTRICTED
    return sid, UNKNOWN


# Jurisdictions of ported Creative Commons licences, as their names spell them.
CC_PORTS = {
    "united states": "us",
    "australia": "au",
    "england and wales": "uk",
    "scotland": "scotland",
    "france": "fr",
    "italy": "it",
    "canada": "ca",
    "czech republic": "cz",
    "netherlands": "nl",
    "spain": "es",
    "germany": "de",
}


def classify_words(t: str) -> tuple[str, str] | None:
    """Classify licence prose (lower-case) that names a licence in words."""
    cc = "creative commons" in t or re.search(r"\bcc[- ]?(by|zero|0)\b", t)
    if cc:
        if re.search(r"cc[- ]?zero|\bcc0\b|public domain", t):
            # CC0 has only ever had version 1.0.
            return "CC0 1.0", OPEN
        parts = []
        if re.search(r"attribut|\bby\b", t):
            parts.append("by")
        if re.search(r"non[- ]?commercial|\bnc\b", t):
            parts.append("nc")
        if re.search(r"no[- ]?deriv|\bnd\b", t):
            parts.append("nd")
        elif re.search(r"share[- ]?alike|\bsa\b", t):
            parts.append("sa")
        if not parts:
            return None
        if parts[0] != "by":
            parts.insert(0, "?")
        m = re.search(r"\b([1-4]\.\d)\b", t) or re.search(r"\bcc[- ]?by[- ]?([1-4])\b", t)
        version = m.group(1) if m else None
        if version and "." not in version:
            version += ".0"
        # A ported licence names its jurisdiction right after the version
        # ("... Share Alike 3.0 United States License"); 4.0 has no ports.
        # Only that position counts, so a place name elsewhere in the prose
        # is not read as a port.
        port = None
        if version and not version.startswith("4"):
            mj = re.search(
                r"\b" + re.escape(version) + r"\s+(" + "|".join(map(re.escape, CC_PORTS)) + r")\b", t
            )
            if mj:
                port = CC_PORTS[mj.group(1)]
        return cc_label(parts, version, port)
    if "public domain" in t:
        return "Public domain", OPEN
    if "apache" in t and "licen" in t:
        return "Apache-2.0", OPEN
    if re.search(r"\bmit\b", t) and "licen" in t:
        return "MIT", OPEN
    for pattern, name in COPYLEFT:
        if pattern.search(t):
            return name, RESTRICTED
    if "academic free licen" in t:
        return "AFL-3.0", RESTRICTED
    return None


URL_RE = re.compile(r"https?://[^\s<>\"'\)\]\[,]+")
LICENSE_WORDS = re.compile(
    r"licen[cs]|creative commons|terms of use|permission|copyleft|public domain|all rights"
)


def classify_value(kind: str, value: str) -> list[tuple[str, str]]:
    """Classify one declared licence/rights value into (label, class) pairs."""
    if kind == "iri":
        hit = classify_iri(value)
        return [hit if hit else (value, UNKNOWN)]
    text = value.strip()
    if not text:
        return []
    bare = text.strip("<>").strip()
    if re.fullmatch(r"https?://\S+", bare):
        # A literal that is only a URL is a licence reference.
        hit = classify_iri(bare)
        return [hit if hit else (bare, UNKNOWN)]
    t = norm_ws(text).lower()
    out = []
    if "all rights reserved" in t:
        out.append(("All rights reserved", RESTRICTED))
    # A URL naming a known licence is more precise than the prose around it;
    # other URLs in prose (project homepages) say nothing about the licence.
    urls = [classify_iri(u.rstrip(".;:")) for u in URL_RE.findall(text)]
    urls = [u for u in urls if u]
    if urls:
        return out + urls
    hit = classify_words(t)
    if hit:
        return out + [hit]
    if out:
        return out
    if LICENSE_WORDS.search(t):
        return [(text, UNKNOWN)]
    return [(text, NOTICE)]


# ─── Publisher terms stated outside the graph ────────────────────────────────
#
# Some publishers state the terms of their vocabularies on a licence page, in
# a specification's footer, in a site-wide notice or in prose that no licence
# predicate carries, not as a licence statement in the vocabulary graph.  This
# table records those terms.  Every row was checked by hand on 2026-09-23
# against a primary source from the rights holder, quoted in the comment.
#
# A row applies only when the vocabulary's own graph names no licence
# (license_status "none" or "copyright-only").  A licence the graph states —
# restrictive, unrecognised or open — always wins.
#
#   match    ("prefix", "<LOV prefix>") for one vocabulary, or
#            ("namespace", "<IRI prefix>") for a publisher's family: the
#            vocabulary's graph IRI and its namespace must both fall under it
#            (http and https alike).  A prefix row wins over a namespace row,
#            a longer namespace over a shorter one.
#   license  licence IRIs (classified by classify_iri, so the labels are the
#            ones used for in-graph statements) or (label, class) pairs.
#   url      the primary source establishing the terms.
#   notice   the statement the terms require on copies, verbatim; None when
#            they ask only for the usual attribution; W3C_DOCUMENT_NOTICE or
#            OGC_DOCUMENT_NOTICE for the document notices, which are built
#            per graph (see w3c_document_notice and GRAPH_NOTICES).
#
# Checked and left without publisher terms (no statement from the rights
# holder found, or one that does not cover the vocabulary as LOV holds it):
# void, dul and the other ontologydesignpatterns.org patterns (cpa, odpart,
# irw, situ, infor, iol, lmm1, seq, ti), rdvocab.info's legacy RDA element
# sets (rdafrbr, rdag1, rdag2, rdarel, rdarole), scovo, spin and sp (the SPIN
# W3C Member Submission is a W3C document; the spinrdf.org files are not),
# ext, dq and li (ISO 19115), dr and coll (SWAN), tvc (the current SPAR
# release is CC BY 4.0; this 2012 version states nothing), swrc, mads and
# premis (www.loc.gov), ov, spatial and ngeo (geovocab.org no longer belongs
# to the publisher), lvont (Lexvo's CC BY-SA covers its data dumps), gold,
# crm, ore (CC BY-SA 3.0 covers the ORE specification pages, not the terms
# document), ao, olo, rel, wot, ov (unreachable) and doap (the repository's
# Apache-2.0 grant is not shown to cover all of LOV's copy; see
# frontend/public/vocab/NOTICE.md).

MOD_NOTICE = (
    "MOD: Metadata for Ontology Description and publication. Creators: Biswanath Dutta, "
    "Clement Jonquet. Rights holder: Indian Statistical Institute and University of "
    "Montpellier."
)

COPYRIGHT_RE = re.compile(r"copyright|©|\(c\)", re.I)

# The OGC Document Notice's form for a document without its own notice.
OGC_FALLBACK = (
    "Original document: {uri}. Copyright © Open Geospatial Consortium, Inc. All Rights "
    "Reserved. OGC Documents. https://www.ogc.org/about-ogc/policies/document-notice/"
)

# Markers for the notices built per graph.
W3C_DOCUMENT_NOTICE = "<built per graph: W3C Document License>"
OGC_DOCUMENT_NOTICE = "<built per graph: OGC Document Notice>"

PUBLISHER_TERMS = [
    {
        # "Unless otherwise stated, documents on the W3C site are published
        # under the W3C document license", and "Public documents on the W3C
        # site are provided by the copyright holders under the following
        # license" (the Document License, 2023 version).  It allows copying
        # and distributing the contents in any medium, with a link to the
        # original, the copyright notice and, where one exists, the
        # document's status on every copy.  Only namespaces W3C serves from
        # its own site.
        "match": ("namespace", "http://www.w3.org/"),
        "license": ["https://www.w3.org/copyright/document-license-2023/"],
        "url": "https://www.w3.org/copyright/intellectual-rights/",
        "notice": W3C_DOCUMENT_NOTICE,
    },
    {
        # ODRL Vocabulary & Expression 2.2, W3C Recommendation 15 February
        # 2018 (https://www.w3.org/TR/2018/REC-odrl-vocab-20180215/): "W3C
        # liability, trademark and permissive document license rules apply",
        # the last linking to the W3C Software and Document License (2015);
        # the Recommendation formalises the vocabulary as ODRL22.ttl.  The
        # w3c/poe repository's LICENSE.md applies the same licence.  The
        # graph's own dcterms:license points to W3C's 2002 IPR notice, an
        # index of policies rather than a licence (kept in license_declared).
        # The notice is the one the licence gives for copies that change the
        # format, as in frontend/public/vocab/NOTICE.md.
        "match": ("prefix", "odrl"),
        "license": ["https://www.w3.org/Consortium/Legal/2015/copyright-software-and-document"],
        "url": "https://www.w3.org/TR/2018/REC-odrl-vocab-20180215/",
        "notice": (
            "This software or document includes material copied from or derived from "
            "ODRL Version 2.2, http://www.w3.org/ns/odrl/2/. Copyright © 2018 W3C® "
            "(MIT, ERCIM, Keio, Beihang)."
        ),
    },
    {
        # DCMI: "Unless indicated otherwise, RDF and XML schemas that are made
        # available on the DCMI Web site ... are licensed under a Creative
        # Commons Attribution 4.0 International License", with the notice
        # below for software that uses them.
        "match": ("namespace", "http://purl.org/dc/"),
        "license": ["https://creativecommons.org/licenses/by/4.0/"],
        "url": "https://www.dublincore.org/about/copyright/",
        "notice": (
            "Portions of this software may use RDF schemas Copyright © 2011 DCMI, the "
            "Dublin Core™ Metadata Initiative. These are licensed under the Creative "
            "Commons 4.0 Attribution license."
        ),
    },
    {
        # "RDA Vocabularies and RDA Registry are licensed under a Creative
        # Commons Attribution 4.0 International License."
        "match": ("namespace", "http://rdaregistry.info/"),
        "license": ["https://creativecommons.org/licenses/by/4.0/"],
        "url": "https://www.rdaregistry.info/",
        "notice": (
            "Copyright © 2020 American Library Association, Canadian Federation of "
            "Library Associations, and CILIP: Chartered Institute of Library and "
            "Information Professionals"
        ),
    },
    {
        # OGC Document Notice: "Public documents on the OGC site are provided
        # by the copyright holders under the following license" — use, copy
        # and distribute the contents in any medium, with a link to the
        # original, the pre-existing copyright notice and, where one exists,
        # the status on every copy; "No right to create modifications or
        # derivatives of OGC documents is granted".  (gsp states its own
        # licence.)  The notices of sf and gml are in GRAPH_NOTICES.
        "match": ("namespace", "http://www.opengis.net/"),
        "license": ["https://www.ogc.org/about/policies/document-notice/"],
        "url": "https://www.ogc.org/about/policies/document-notice/",
        "notice": OGC_DOCUMENT_NOTICE,
    },
    {
        # id.loc.gov: "The Library of Congress has prepared this linked data
        # system and is making it available as a public domain data set."
        "match": ("namespace", "http://id.loc.gov/"),
        "license": [("Public domain", OPEN)],
        "url": "https://id.loc.gov/about/",
        "notice": None,
    },
    {
        # ESCO FAQ: "In accordance with the Commission Decision of 12 December
        # 2011 on the reuse of Commission documents (2011/833/EU), the ESCO
        # classification can be downloaded, used, reproduced and reused for
        # any purpose and by any interested party free of charge", on two
        # conditions: publish the statement below, and mark any modified or
        # adapted version of ESCO as such.
        "match": ("namespace", "http://data.europa.eu/esco/"),
        "license": [("Commission reuse policy (Decision 2011/833/EU)", OPEN)],
        "url": "https://esco.ec.europa.eu/en/about-esco/faq",
        "notice": "This service uses the ESCO classification of the European Commission.",
    },
    {
        # NEPOMUK / OSCAF ontologies: "This work is made available under the
        # terms of Nepomuk software license and the OSCAF License" — a
        # 3-clause BSD licence (LICENSE.txt) and "licensed under either CC-BY
        # or BSD".  Full text: LICENSES/BSD-3-Clause-NEPOMUK.txt.
        "match": ("namespace", "http://www.semanticdesktop.org/ontologies/"),
        "license": ["https://opensource.org/licenses/BSD-3-Clause"],
        "url": "https://www.semanticdesktop.org/ontologies/",
        # LICENSE.txt's copyright notice (its two lines joined), then the
        # site's line for OSCAF's later contributions ("Copyright © 2009-2014
        # OSCAF and contributors", page footer).
        "notice": (
            "Copyright (c) 2006-2008, NEPOMUK consortium and contributors. All rights "
            "reserved. Copyright © 2009-2014 OSCAF and contributors. "
            "https://www.semanticdesktop.org/ontologies/LICENSE.txt"
        ),
    },
    {
        # "This work is licensed under a Creative Commons Attribution License
        # [by/1.0]. This copyright applies to the FOAF Vocabulary
        # Specification and accompanying documentation in RDF."
        "match": ("prefix", "foaf"),
        "license": ["http://creativecommons.org/licenses/by/1.0/"],
        "url": "http://xmlns.com/foaf/spec/",
        "notice": "Copyright © 2000-2014 Dan Brickley and Libby Miller",
    },
    {
        # "The Sponsors' copyrights in the schema are licensed to website
        # publishers and other third parties under the Creative Commons
        # Attribution-ShareAlike License (version 3.0)."
        "match": ("prefix", "schema"),
        "license": ["http://creativecommons.org/licenses/by-sa/3.0/"],
        "url": "https://schema.org/docs/terms.html",
        "notice": None,
    },
    {
        # Creative Commons: "all content on this site is licensed under the
        # Creative Commons Attribution 4.0 International license unless
        # otherwise marked" (the ccREL schema is served from that site).
        "match": ("prefix", "cc"),
        "license": ["https://creativecommons.org/licenses/by/4.0/"],
        "url": "https://creativecommons.org/policies/",
        "notice": None,
    },
    {
        # BIBO is a DCMI Community Specification: "Unless indicated
        # otherwise, DCMI documents are licensed under a Creative Commons
        # Attribution 4.0 International License."
        "match": ("prefix", "bibo"),
        "license": ["https://creativecommons.org/licenses/by/4.0/"],
        "url": "https://www.dublincore.org/specifications/bibo/",
        "notice": None,
    },
    {
        # DCMI hosts the SKOS-Thes namespace; its site licenses DCMI
        # documents and the RDF schemas it serves under CC BY 4.0.
        "match": ("prefix", "iso-thes"),
        "license": ["https://creativecommons.org/licenses/by/4.0/"],
        "url": "https://www.dublincore.org/specifications/skos-thes/",
        "notice": None,
    },
    {
        # "This work is licensed under a Creative Commons License [by/1.0].
        # This copyright applies to the SIOC Core Ontology Specification and
        # accompanying documentation".
        "match": ("prefix", "sioc"),
        "license": ["http://creativecommons.org/licenses/by/1.0/"],
        "url": "http://rdfs.org/sioc/spec/",
        # LOV holds revision 1.35 (2010), whose specification carried this
        # notice; the current one names the Data Science Institute (formerly
        # DERI) for 2004-2018.
        "notice": "Copyright © 2004-2010 by DERI, NUI Galway",
    },
    {
        # "This work is licensed under a Creative Commons Attribution License
        # [by/1.0]. This copyright applies to the Event Ontology and
        # accompanying documentation in RDF."
        "match": ("prefix", "event"),
        "license": ["http://creativecommons.org/licenses/by/1.0/"],
        "url": "http://motools.sourceforge.net/event/event.html",
        "notice": None,
    },
    {
        # "This work is licensed under a Creative Commons License [by/1.0].
        # This copyright applies to the Music Ontology Specification and
        # accompanying documentation".
        "match": ("prefix", "mo"),
        "license": ["http://creativecommons.org/licenses/by/1.0/"],
        "url": "http://motools.sourceforge.net/doc/musicontology.html",
        "notice": None,
    },
    {
        # UniProt: "We have chosen to apply the Creative Commons Attribution
        # 4.0 International (CC BY 4.0) License to all copyrightable parts of
        # our databases" (the core ontology ships with the UniProt RDF).
        "match": ("prefix", "uniprot"),
        "license": ["https://creativecommons.org/licenses/by/4.0/"],
        "url": "https://www.uniprot.org/help/license",
        "notice": None,
    },
    {
        # MOD 2.0 as its publisher releases it states dcterms:license CC BY
        # 4.0 ("MOD is licensed under the Creative Commons Attribution 4.0");
        # LOV's copies (at both namespaces) lack that triple.  CC BY asks for
        # the creators and the rights holder: the same file's dcterms:creator
        # and dcterms:rightsHolder (checked 2026-09-23).
        "match": ("prefix", "modp"),
        "license": ["https://creativecommons.org/licenses/by/4.0/"],
        "url": "https://github.com/FAIR-IMPACT/MOD/blob/main/versions/2.0/mod-v2.0.ttl",
        "notice": MOD_NOTICE,
    },
    {
        "match": ("prefix", "mod"),
        "license": ["https://creativecommons.org/licenses/by/4.0/"],
        "url": "https://github.com/FAIR-IMPACT/MOD/blob/main/versions/2.0/mod-v2.0.ttl",
        "notice": MOD_NOTICE,
    },
    {
        # OMG's authors (Wagner, Bonduel, Pauwels): the documentation that
        # https://w3id.org/omg resolves to (annawagner.github.io/omg/) gives
        # "License:" CC BY 4.0, and its schema.org JSON-LD has "version":
        # "0.0.1", "license":"https://creativecommons.org/licenses/by/4.0/";
        # their Widoco configuration (github.com/tudaIIB/omg, config.config)
        # has ontologyRevisionNumber=0.0.1 and licenseURI CC BY 4.0.  LOV
        # holds that version (owl:versionInfo "0.0.1"; the graph itself has no
        # licence triple).  The authors are as the documentation names them.
        "match": ("prefix", "omg"),
        "license": ["https://creativecommons.org/licenses/by/4.0/"],
        "url": "https://w3id.org/omg",
        "notice": (
            "OMG: Ontology for Managing Geometry, by Anna Wagner (TU Darmstadt), "
            "Mathias Bonduel (KU Leuven) and Pieter Pauwels (Ghent University)."
        ),
    },
    {
        # Stated in prose in the graph (rdfs:comment of the ontology): "The
        # Erlangen CRM / OWL implementation of the CIDOC Conceptual Reference
        # Model is licensed under a Creative Commons Attribution-ShareAlike
        # 3.0 Unported License."
        "match": ("prefix", "ecrm"),
        "license": ["http://creativecommons.org/licenses/by-sa/3.0/"],
        "url": "http://erlangen-crm.org/current/",
        "notice": None,
    },
    {
        # Stated in prose in the graph (vaem:withAttributionTo): "VAEM is
        # issued under a Creative Commons Attribution Share Alike 3.0 United
        # States License. Attribution should be made to TopQuadrant, Inc."
        "match": ("prefix", "vaem"),
        "license": ["http://creativecommons.org/licenses/by-sa/3.0/us/"],
        "url": "http://www.linkedmodel.org/schema/vaem",
        "notice": "Attribution should be made to TopQuadrant, Inc.",
    },
    {
        # Stated in prose in the graph (rdfs:comment): "This work is licensed
        # under a Creative Commons Attribution-ShareAlike 4.0 International
        # License."
        "match": ("prefix", "cart"),
        "license": ["http://creativecommons.org/licenses/by-sa/4.0/"],
        "url": "http://purl.org/net/cartCoord",
        "notice": None,
    },
    {
        # The graph says its use "is governed by FAO's copyright
        # reservation"; FAO's terms allow copying "for private study,
        # research and teaching purposes, and for use in non-commercial
        # products or services" only.
        "match": ("prefix", "geop"),
        "license": [("FAO copyright reservation (non-commercial use only)", RESTRICTED)],
        "url": "https://www.fao.org/contact-us/terms/en/",
        "notice": None,
    },
]


# ─── Notices per graph ───────────────────────────────────────────────────────
#
# Statements a licence requires on copies that the graph does not carry
# itself, keyed by graph IRI and quoted from the rights holder (source in the
# comment; all checked on 2026-09-23).  One replaces any other notice for the
# graph, publisher-terms or built, and is recorded in license_notice.


def _mit_bsd(line: str, source: str) -> str:
    return f"{line}. Licence file: {source}"


_POSO = "Copyright (c) 2021-2025 Maxim Van de Wynckel, Beat Signer & Vrije Universiteit Brussel"
_MDO = "Copyright (C) 2019 Huanyu Li"
_MDO_SRC = "https://github.com/huanyu-li/Materials-Design-Ontology/blob/master/LICENSE"
_SAREF_SRC = "https://labs.etsi.org/rep/saref/{repo}/-/blob/develop-v{version}/LICENSE"
_SWAP = (
    "Share and Enjoy. Open Source license: Copyright (c) 2000,1,2 W3C (MIT, INRIA, Keio) "
    "Copyright other contributers mentioned in individual files. "
    "http://www.w3.org/Consortium/Legal/copyright-software-19980720 "
    "(http://www.w3.org/2000/10/swap/LICENSE.n3). Changes: none by Open Triplestore; LOV "
    "serialized the file as N-Quads."
)
_OGC_GEOSPARQL_10 = (
    "Original document: {source}. GeoSPARQL 1.0 is an OGC Standard. Copyright (c) 2012 "
    "Open Geospatial Consortium. To obtain additional rights of use, visit "
    "http://www.opengeospatial.org/legal/ . Version: 1.0.1. OGC Document Notice: "
    "https://www.ogc.org/about-ogc/policies/document-notice/"
)
# ISA Open Metadata Licence v1.1 (LICENSES/ISA-Open-Metadata-Licence-1.1.txt):
# distributions must retain the copyright notice (§3(a)) and the licence's
# "No Warranty" disclaimer (§3(b)), and link to Joinup where practical (§3(c)).
# The disclaimer is §4 verbatim, its two paragraphs joined by a space (a
# notice is one line: the allowlist is tab-separated, one graph per line).
_ISA_URL = "https://interoperable-europe.ec.europa.eu/licence/isa-open-metadata-licence-v11"
_ISA_DISCLAIMER = (
    'No Warranty: EACH WORK IS PROVIDED "AS IS" WITHOUT REPRESENTATIONS, WARRANTIES, '
    "OBLIGATIONS AND LIABILITIES OF ANY KIND, EITHER EXPRESS OR IMPLIED, TO THE FULL EXTENT "
    "PERMITTED BY LAW INCLUDING, BUT NOT LIMITED TO, ANY IMPLIED WARRANTY OF MERCHANTABILITY, "
    "INTEGRATION, SATISFACTORY QUALITY AND FITNESS FOR A PARTICULAR PURPOSE. EXCEPT IN THE "
    "CASES OF WILFUL MISCONDUCT OR DAMAGES DIRECTLY CAUSED TO NATURAL PERSONS, NEITHER "
    "EUROPEAN UNION NOR ITS CONTRIBUTOR(S) WILL BE LIABLE FOR ANY INCIDENTAL, CONSEQUENTIAL, "
    "DIRECT OR INDIRECT DAMAGES INCLUDING BUT NOT LIMITED TO THE LOSS OF DATA, LOST PROFITS, "
    "OR ANY OTHER FINANCIAL LOSS ARISING FROM THE USE OF, OR INABILITY TO USE, EVEN IF THE "
    "EUROPEAN UNION HAS BEEN NOTIFIED OF THE POSSIBILITY OF SUCH LOSS, DAMAGES, CLAIMS OR "
    "COSTS OR FOR ANY CLAIM BY ANY THIRD PARTY. HOWEVER, THE LICENSOR WILL BE LIABLE UNDER "
    "STATUTORY PRODUCT LIABILITY LAWS AS FAR SUCH LAWS APPLY TO THE WORK."
)
_ISA_TERMS = f"Licensed under the ISA Open Metadata Licence v1.1, {_ISA_URL} (Joinup: https://joinup.ec.europa.eu/)."

GRAPH_NOTICES = {
    # MIT.  POSO: http://purl.org/poso/ and .../common/ resolve to
    # openhps.github.io/POSO, published from github.com/OpenHPS/POSO, whose
    # LICENSE is MIT with this line.
    "http://purl.org/poso/": _mit_bsd(_POSO, "https://github.com/OpenHPS/POSO/blob/master/LICENSE"),
    "http://purl.org/poso/common/": _mit_bsd(
        _POSO, "https://github.com/OpenHPS/POSO/blob/master/LICENSE"
    ),
    # EUTaxO: https://w3id.org/EUTaxO resolves to jfaldanam.gitlab.io/EUTaxO,
    # published from gitlab.com/jfaldanam/EUTaxO (EUTaxO.owl), LICENSE MIT.
    "https://w3id.org/EUTaxO": _mit_bsd(
        "Copyright (c) 2021 jfaldanam", "https://gitlab.com/jfaldanam/EUTaxO/-/blob/master/LICENSE"
    ),
    # OPTiMAR: https://w3id.org/optimar resolves to
    # bisite.github.io/OPTIMAR-Ontology, published from
    # github.com/BISITE/OPTIMAR-Ontology, LICENSE MIT.
    "https://w3id.org/optimar": _mit_bsd(
        "Copyright (c) 2025 USAL", "https://github.com/BISITE/OPTIMAR-Ontology/blob/main/LICENSE"
    ),
    # MDO: the licence file each graph's own dcterms:license names.
    "https://w3id.org/mdo/calculation/": _mit_bsd(_MDO, _MDO_SRC),
    "https://w3id.org/mdo/core/": _mit_bsd(_MDO, _MDO_SRC),
    "https://w3id.org/mdo/full/": _mit_bsd(_MDO, _MDO_SRC),
    "https://w3id.org/mdo/provenance/": _mit_bsd(_MDO, _MDO_SRC),
    "https://w3id.org/mdo/structure/": _mit_bsd(_MDO, _MDO_SRC),
    # BSD-3-Clause.  SAREF: the graphs' dcterms:license
    # forge.etsi.org/etsi-software-license puts ETSI Forge repositories under
    # BSD-3-Clause; each repository's LICENSE, on the branch of the version
    # LOV holds (owl:versionInfo), carries the line below.
    **{
        f"https://saref.etsi.org/{ns}/": _mit_bsd(
            f"Copyright {year} ETSI", _SAREF_SRC.format(repo=repo, version=version)
        )
        for ns, repo, version, year in [
            ("core", "saref-core", "3.1.1", 2019),
            ("saref4agri", "saref4agri", "1.1.2", 2020),
            ("saref4bldg", "saref4bldg", "1.1.2", 2020),
            ("saref4city", "saref4city", "1.1.2", 2020),
            ("saref4ehaw", "saref4ehaw", "1.1.1", 2019),
            ("saref4ener", "saref4ener", "1.1.2", 2019),
            ("saref4envi", "saref4envi", "1.1.2", 2020),
            ("saref4inma", "saref4inma", "1.1.2", 2019),
            ("saref4syst", "saref4syst", "1.1.2", 2019),
            ("saref4watr", "saref4watr", "1.1.1", 2019),
            ("saref4wear", "saref4wear", "1.1.1", 2019),
        ]
    },
    # CC BY-SA 3.0 United States.  The oeGOV graphs cc and gc state that
    # licence and name the party to credit, cc:attributionName "TopQuadrant,
    # Inc."; CC BY-SA 3.0 also asks for the licence's URI with every copy.
    **{
        f"http://www.oegov.org/core/owl/{name}": (
            "Attribution: TopQuadrant, Inc. (the graph's cc:attributionName). Licence: "
            "https://creativecommons.org/licenses/by-sa/3.0/us/"
        )
        for name in ("cc", "gc")
    },
    # W3C Software Notice (1998).  doc and gso name LICENSE.n3 (doc:ipr);
    # its comment is the pre-existing notice, quoted whole.  Their triples
    # match W3C's files apart from the $Id line, which shows LOV took the
    # RDF/XML variants (doc.rdf, ont.rdf).
    "http://www.w3.org/2000/10/swap/pim/doc": _SWAP,
    "http://www.w3.org/2006/gen/ont": _SWAP,
    # OGC Document Notice.  sf and gml are, triple for triple, the OGC's
    # simple_features_geometries.rdf and gml_32_geometries.rdf; the notice
    # is the XML comment at the top of each file, which LOV's copy drops.
    "http://www.opengis.net/ont/sf": _OGC_GEOSPARQL_10.format(
        source="http://schemas.opengis.net/sf/1.0/simple_features_geometries.rdf"
    ),
    "http://www.opengis.net/ont/gml": _OGC_GEOSPARQL_10.format(
        source="http://schemas.opengis.net/gml/3.2.1/gml_32_geometries.rdf"
    ),
    # ISA Open Metadata Licence v1.1, named by each graph's dcterms:license:
    # the graph's own copyright line (dcterms:rights; person's literal writes
    # the sign as the HTML entity "&#169;", localgov has a mis-decoded copy
    # "Copyright Â© 2013-2014 V-ICT-OR" next to the one quoted), the licence
    # and its disclaimer, which none of these graphs carries.
    **{
        uri: f"{rights} {_ISA_TERMS} {_ISA_DISCLAIMER}"
        for uri, rights in [
            ("http://www.w3.org/ns/locn", "Copyright © European Union, 2012-2015."),
            ("http://www.w3.org/ns/person", "Copyright © 2012 European Commission."),
            ("http://www.w3.org/ns/radion#", "Copyright © 2012 European Commission."),
            ("http://purl.org/oslo/ns/localgov", "Copyright (c) 2013-2014 V-ICT-OR."),
        ]
    },
    # UK Open Government Licence, named unversioned by the graph
    # (cc:license <http://www.nationalarchives.gov.uk/doc/open-government-licence>,
    # which resolves to version 3.0): "If the Information Provider does not
    # provide a specific attribution statement, you must use the following"
    # statement, with a link to the licence where possible.  reegle states
    # none.
    "http://reegle.info/schema": (
        "Contains public sector information licensed under the Open Government Licence v3.0. "
        "https://www.nationalarchives.gov.uk/doc/open-government-licence/version/3/"
    ),
    # Modellicentie Gratis Hergebruik Vlaanderen v1.0, art. 4: the attribution
    # the licensor specifies or, lacking one, the licensor's name and, where
    # possible, the year the product was made available.  The graph names
    # none; it gives dcterms:mediator "Data Vlaanderen" and dcterms:issued
    # 2017-03-31.  (Licence text: the archived copy of 2023-05-24.)
    "http://data.vlaanderen.be/ns/persoon": (
        "Bron: Data Vlaanderen (the vocabulary's dcterms:mediator), 2017. Modellicentie Gratis "
        "Hergebruik Vlaanderen v1.0: https://overheid.vlaanderen.be/sites/default/files/"
        "documenten/ict-egov/licenties/hergebruik/modellicentie_gratis_hergebruik_v1_0.html"
    ),
    # CC BY 1.0 asks copies to keep the copyright notices intact; the graph's
    # only one is mis-decoded in LOV's copy ("Copyright Â© 2015 Tourpedia").
    # The publisher's tp.owl reads as quoted (checked 2026-09-23).
    "http://tour-pedia.org/download/tp.owl": "Copyright © 2015 Tourpedia",
}

# Added after a graph's licence notice: material under a second licence.
_ISA = (
    "Copyright © 2012 European Union. This vocabulary is published under the ISA Open "
    f"Metadata Licence v1.1 ({_ISA_URL}). {_ISA_DISCLAIMER}"
)
EXTRA_NOTICES = {
    # ADMS's definitions derive from ADMS v1.00 of the EU ISA Programme (the
    # graph: "originally developed under the European Union's ISA
    # Programme"); that release's page (interoperable-europe.ec.europa.eu,
    # .../asset-description-metadata-schema-adms/release/100) says the line
    # below, as frontend/public/vocab/NOTICE.md records for adms.ttl.
    "http://www.w3.org/ns/adms": "It derives from ADMS v1.00 of the ISA Programme: " + _ISA,
    # RegOrg: "the RDF encoding of the Legal Entity vocabulary, originally
    # developed under the European Commission's ISA Programme" (the graph),
    # i.e. the Core Business Vocabulary 1.00, whose Joinup release page
    # (joinup.ec.europa.eu/asset/core_business/asset_release/
    # core-business-vocabulary-100) says the line below.
    "http://www.w3.org/ns/regorg": (
        "It derives from the Core Business Vocabulary 1.00 of the ISA Programme: " + _ISA
    ),
}

# The original of a W3C document, where its IRI no longer serves it:
# http://www.w3.org/ns/adms now redirects to SEMIC's ADMS 2.00; the revision
# LOV holds (2015-07-22) is W3C's legacy_adms, triple for triple.
LINK_OVERRIDES = {"http://www.w3.org/ns/adms": "https://www.w3.org/ns/legacy_adms"}

# Graphs whose licence would allow redistribution, withheld because LOV's copy
# is not a faithful copy of a work that allows no modification.
WITHHELD = {
    # CC BY-ND 4.0.  Against the publisher's file (purl.org/drammar ->
    # cirma.unito.it/drammar/drammar.owl): 1,348 triples each, but two
    # literals differ where LOV mis-decoded UTF-8 (drammar's dc:description
    # "so–called" and drammar#Agent's comment "resource‐bounded").
    "http://www.purl.org/drammar": (
        "LOV's copy is not a faithful copy of the publisher's drammar.owl: two literals were "
        "mis-decoded, and CC BY-ND 4.0 allows no modified copies."
    ),
    # The graph's only licence statement is dc:rights "Creative Commons - By
    # Attribution", with no version.  Every version of CC BY requires each
    # copy to carry the licence's URI or text (1.0 to 3.0: "a copy of, or the
    # Uniform Resource Identifier for, this License"; 4.0: the licence or its
    # URI), and the publisher does not say which one applies.
    "http://www.w3.org/2001/sw/hcls/ns/transmed/": (
        "Its graph names Creative Commons Attribution without a version (\"Creative Commons - "
        "By Attribution\"), and every version of that licence requires copies to carry the URI "
        "or text of the licence that applies, which the publisher does not identify."
    ),
}

# Mis-decoded text (see mojibake()) that is the publisher's own, not LOV's:
# LOV's copy then matches the publisher's document there.
UPSTREAM_MOJIBAKE = {
    # ssn:DetectionLimit's comment ("... is Î², given a probability Î± ...")
    # is word for word the one in W3C's own SSN-XG ontology,
    # https://www.w3.org/2005/Incubator/ssn/ssnx/ssn.owl.
    "https://www.w3.org/ns/ssn",
    # The same ssnx:DetectionLimit comment, in the W3C/OGC SSN graph.
    "http://www.w3.org/ns/ssn/",
}

# Graphs whose mis-decoded text was checked against the publisher's own file
# (fetched 2026-09-23), which has the correct characters where LOV's copy has
# the mis-decoded ones: LOV introduced the errors.  For the other flagged
# graphs the notice says only that the copy holds mis-decoded text.
LOV_MISDECODING_CHECKED = {
    "http://data.ign.fr/def/topo": "http://data.ign.fr/def/topo (RDF/XML)",
    "http://rdf.muninn-project.org/ontologies/military": (
        "http://rdf.muninn-project.org/ontologies/military (RDF/XML)"
    ),
    "https://w3id.org/HHT": "https://w3id.org/HHT (RDF/XML)",
    "https://w3id.org/vpa": "https://linkedvocabs.org/onto/vpa/ontology.owl",
    "http://purl.org/oslo/ns/localgov": "http://purl.org/oslo/ns/localgov (RDF/XML)",
    "http://tour-pedia.org/download/tp.owl": "http://tour-pedia.org/download/tp.owl",
}

# Character runs mojibake() matches that are real text: the multiplication
# sign before a no-break space ("4.8 × 10", in EMMO's SquareDegree comment).
NOT_MOJIBAKE = {"\u00d7\u00a0"}

# Licences whose conditions include keeping the copyright notice on every
# copy; a graph under one of them ships only with that notice.
COPYRIGHT_LINE_LICENCES = {"MIT", "BSD-2-Clause", "BSD-3-Clause", "ISC"}

# UTF-8 read as Latin-1 or Windows-1252: a lead byte (Â-ô) followed by one or
# more continuation bytes, in either code page.
_CP1252_HIGH = "ŒœŠšŸŽžƒˆ˜–—‘’‚“”„†‡•…‰‹›€™"
MOJIBAKE_RE = re.compile("[Â-ô][\u0080-¿" + _CP1252_HIGH + "]+")


def mojibake(text: str) -> str | None:
    """A run of `text` that is UTF-8 mis-decoded as Latin-1 or Windows-1252."""
    for m in MOJIBAKE_RE.finditer(text):
        run = m.group(0)
        if run in NOT_MOJIBAKE:
            continue
        for enc in ("latin-1", "cp1252"):
            try:
                if run.encode(enc).decode("utf-8") != run:
                    return run
            except UnicodeError:
                continue
    return None


# ─── W3C document notices ────────────────────────────────────────────────────
#
# The W3C Document License (2023; 2015 for graphs that name that version)
# lets anyone copy and distribute a document's contents in any medium, with
# on every copy: a link to the original, the pre-existing copyright notice or
# else one "of the form" given below, and the document's STATUS where one
# exists.  LOV re-serialized these documents as N-Quads, so the notice also
# carries the one the licence prescribes for material "copied from or derived
# from" a document in software, with the document's title and URI; it holds
# either way.  Both forms are quoted from the licence texts
# (https://www.w3.org/copyright/document-license-2023/ and -2015/, fetched
# 2026-09-23): only the bracketed slots are filled in, with the year of the
# document (w3c_document_year), its title and IRI (the graph's own), and the
# status the graph itself states, if any (w3c_document_status).

W3C_COPY_FORM = {
    "2023": "Copyright © {year} World Wide Web Consortium. https://www.w3.org/copyright/document-license-2023/",
    "2015": (
        "Copyright © {year} World Wide Web Consortium, (MIT, ERCIM, Keio, Beihang). "
        "https://www.w3.org/copyright/document-license-2015/"
    ),
}
W3C_DERIVATIVE_FORM = {
    "2023": (
        "Copyright © 2023 W3C®. This software or document includes material copied from or "
        "derived from {title}, {uri}."
    ),
    "2015": (
        "Copyright © 2015 W3C® (MIT, ERCIM, Keio, Beihang). This software or document includes "
        "material copied from or derived from {title}, {uri}."
    ),
}

_MONTHS = "january|february|march|april|may|june|july|august|september|october|november|december"
_DATE_RES = [
    re.compile(r"\b((?:19|20)\d\d)[-/.](?:0[1-9]|1[0-2])[-/.](?:0[1-9]|[12]\d|3[01])"),
    re.compile(r"\b(?:\d{1,2}\s+)?(?:" + _MONTHS + r")\s+((?:19|20)\d\d)\b", re.I),
    re.compile(r"^\s*((?:19|20)\d\d)\s*$"),
]
_IRI_DATE_RES = [
    re.compile(r"(?<!\d)((?:19|20)\d\d)(?:0[1-9]|1[0-2])(?:0[1-9]|[12]\d|3[01])(?!\d)"),
    re.compile(r"/((?:19|20)\d\d)/\d\d/"),
]
# The maturity levels W3C gives its documents.
W3C_STATUS_RE = re.compile(
    r"recommendation|working draft|editor'?s (?:working )?draft|group note|working group note|"
    r"interest group note|member submission|team submission|community group|\bnote\b",
    re.I,
)


def _years(values, patterns) -> list[int]:
    out = []
    for v in values:
        for pat in patterns:
            out += [int(y) for y in pat.findall(v)]
    return out


def _merged(nodes: list[dict]) -> dict:
    out = defaultdict(list)
    for node in nodes:
        for p, vals in node.items():
            out[p].extend(vals)
    return out


def w3c_document_year(nodes: list[dict], versions: list, uri: str) -> tuple[int, str]:
    """The year of the document LOV holds, and where it comes from.

    The graph's own dates first — dc/dcterms modified, date and issued, a date
    in owl:versionInfo (e.g. "$Date: 2009/11/15 ..." or "Working Draft 29
    April 2011") or in owl:versionIRI (prov-20130430) — the latest of them;
    then dcterms/dc:created; then LOV's date for the version it holds; then
    the /YYYY/ in a www.w3.org IRI.
    """
    node = _merged(nodes)
    lits = lambda preds: [v for p in preds for k, v, _ in node.get(p, []) if k == "lit"]
    iris = lambda preds: [v for p in preds for k, v, _ in node.get(p, []) if k == "iri"]
    own = _years(lits(NODE_DATES) + lits(NODE_VERSION_INFO), _DATE_RES) + _years(
        iris(NODE_VERSION_IRI), _IRI_DATE_RES
    )
    if own:
        return max(own), "graph"
    created = _years(lits(NODE_CREATED), _DATE_RES)
    if created:
        return max(created), "graph (created)"
    if versions and versions[0].get("issued"):
        return int(versions[0]["issued"][:4]), "LOV version date"
    m = re.search(r"w3\.org/(?:TR/)?((?:19|20)\d\d)/", uri)
    if m:
        return int(m.group(1)), "IRI"
    raise ValueError(f"no year for the W3C document {uri}")


def _status_of(node: dict) -> list[str]:
    out = []
    for p in NODE_VERSION_INFO:
        for k, v, _ in node.get(p, []):
            if k == "lit" and W3C_STATUS_RE.search(v):
                out.append(norm_ws(v))
    # bibo:status as words; a status given only as an IRI (dpv's
    # ".../bibo/status/published") names a publication state, not the
    # document's status.
    for p in NODE_STATUS:
        out += [norm_ws(v) for k, v, _ in node.get(p, []) if k == "lit" and not re.match(r"https?://", v)]
    return list(dict.fromkeys(out))


def w3c_document_status(nodes: list[dict]) -> str | None:
    """The status the graph states for itself: an owl:versionInfo naming a
    W3C maturity level, or bibo:status, on the vocabulary's node — or on its
    only other ontology node.  (A graph that bundles several ontologies, as
    prov does, states their statuses, not its own.)"""
    primary, others = nodes[0], nodes[1:]
    status = _status_of(primary) or (_status_of(others[0]) if len(others) == 1 else [])
    return "; ".join(status) if status else None


def w3c_document_title(nodes: list[dict], fallback: str) -> str:
    """The graph's own title for itself (English or untagged), else LOV's."""
    for node in nodes:
        for p in NODE_TITLE:
            for k, v, lang in node.get(p, []):
                if k == "lit" and norm_ws(v) and (lang in (None, "en") or lang.startswith("en-")):
                    return norm_ws(v)
    return fallback


def w3c_document_notice(version: str, nodes: list[dict], versions: list, uri: str,
                        lov_title: str, own_copyright: list[str]) -> str:
    """The notice a W3C Document License copy of the graph `uri` carries.

    `nodes` is what the graph says about its vocabulary node, that node
    first, then any other ontology nodes it defines."""
    title = w3c_document_title(nodes, lov_title)
    link = LINK_OVERRIDES.get(uri, uri)
    own = own_copyright or ([PREEXISTING_COPYRIGHT[uri]] if uri in PREEXISTING_COPYRIGHT else [])
    if own:
        parts = [" ".join(own)]
    else:
        year, _ = w3c_document_year(nodes, versions, uri)
        parts = [W3C_COPY_FORM[version].format(year=year)]
    status = w3c_document_status(nodes)
    if status:
        parts.append(f"Status: {status}{'' if status.endswith('.') else '.'}")
    parts.append(W3C_DERIVATIVE_FORM[version].format(title=title, uri=link))
    notice = " ".join(parts)
    if "[$" in notice or re.search(r"\{(year|title|uri)\}", notice):
        raise ValueError(f"unfilled placeholder in the notice for {uri}: {notice}")
    return notice

# The copyright notice of a document's author where the graph LOV holds does
# not carry it; the document licences ask for it in place of their own form.
PREEXISTING_COPYRIGHT = {
    # DPV 2.2 (the graph: owl:versionInfo "2.2"), a W3C Community Group
    # report; its specification, https://w3c-cg.github.io/dpv/2.2/dpv/,
    # carries this line.
    "https://w3id.org/dpv": (
        "Copyright © 2025 the Contributors to the Data Privacy Vocabulary (DPV) Specification, "
        "published by the Data Privacy Vocabularies and Controls Community Group under the W3C "
        "Community Final Specification Agreement (FSA)."
    ),
}

def _terms_classes(row: dict) -> list[tuple[str, str]]:
    out = []
    for lic in row["license"]:
        if isinstance(lic, tuple):
            out.append(lic)
            continue
        hit = classify_iri(lic)
        if hit is None or hit[1] == UNKNOWN:
            raise ValueError(f"publisher-terms licence not recognised: {lic}")
        out.append(hit)
    return out


def _under(iri: str, family: str) -> bool:
    strip = lambda s: re.sub(r"^https?://", "", s)
    return strip(iri).startswith(strip(family))


def publisher_terms(prefix: str, uri: str, nsp: str | None) -> dict | None:
    """The publisher-terms row for a vocabulary, if any (see PUBLISHER_TERMS)."""
    for row in PUBLISHER_TERMS:
        kind, value = row["match"]
        if kind == "prefix" and value == prefix:
            return row
    best = None
    for row in PUBLISHER_TERMS:
        kind, value = row["match"]
        if kind == "namespace" and _under(uri, value) and _under(nsp or uri, value):
            if best is None or len(value) > len(best["match"][1]):
                best = row
    return best


# ─── Licence URIs ────────────────────────────────────────────────────────────
#
# The canonical URI of each licence label, recorded per vocabulary in
# license_uris (in the order of license) and spelled out in NOTICE.  A label
# this table does not cover takes the IRI the vocabulary (or its publisher's
# terms) gave for it, if any; the build fails when a shipped graph's licence
# has neither.

_CC_LABEL_RE = re.compile(r"CC (BY(?:-NC)?(?:-ND|-SA)?) (\d\.\d)(?: ([A-Z]+))?")

LICENSE_URIS = {
    "CC0 1.0": "https://creativecommons.org/publicdomain/zero/1.0/",
    "Public Domain Mark 1.0": "https://creativecommons.org/publicdomain/mark/1.0/",
    "CC Public Domain": "http://creativecommons.org/licenses/publicdomain/",
    "ODbL 1.0": "https://opendatacommons.org/licenses/odbl/1-0/",
    "ODC-By 1.0": "https://opendatacommons.org/licenses/by/1-0/",
    "PDDL 1.0": "https://opendatacommons.org/licenses/pddl/1-0/",
    "Licence Ouverte (Etalab)": "https://www.data.gouv.fr/Licence-Ouverte-Open-Licence",
    "Unlicense": "https://unlicense.org/",
    "Apache-2.0": "https://www.apache.org/licenses/LICENSE-2.0",
    "MIT": "https://opensource.org/licenses/MIT",
    "BSD-2-Clause": "https://opensource.org/licenses/BSD-2-Clause",
    "BSD-3-Clause": "https://opensource.org/licenses/BSD-3-Clause",
    "ISC": "https://opensource.org/licenses/ISC",
    "W3C Document License (2023)": "https://www.w3.org/copyright/document-license-2023/",
    "W3C Document License (2015)": "https://www.w3.org/Consortium/Legal/2015/doc-license",
    "W3C Software and Document License (2015)": (
        "https://www.w3.org/Consortium/Legal/2015/copyright-software-and-document"
    ),
    "W3C Software License (2002)": (
        "https://www.w3.org/Consortium/Legal/2002/copyright-software-20021231"
    ),
    "W3C Software Notice (1998)": "http://www.w3.org/Consortium/Legal/copyright-software-19980720",
    "OGC Document Notice": "https://www.ogc.org/about-ogc/policies/document-notice/",
    "ISA Open Metadata Licence 1.1": _ISA_URL,
    # The graph's unversioned IRI resolves to version 3.0.
    "UK Open Government Licence": (
        "https://www.nationalarchives.gov.uk/doc/open-government-licence/version/3/"
    ),
    "Commission reuse policy (Decision 2011/833/EU)": "https://eur-lex.europa.eu/eli/dec/2011/833/oj",
}


# Labels that name a status rather than a licence, so no licence URI: a
# publisher's public-domain statement (id.loc.gov's, cited in
# license_source_url; British Library Terms' "public domain", next to its
# Public Domain Mark IRI).
NO_LICENSE_URI = {"Public domain"}


def license_uri(label: str, declared: dict[str, str]) -> str | None:
    """The URI of a licence label: canonical, else the IRI given for it."""
    if label in LICENSE_URIS:
        return LICENSE_URIS[label]
    m = _CC_LABEL_RE.fullmatch(label)
    if m:
        port = f"{m.group(3).lower()}/" if m.group(3) else ""
        return f"https://creativecommons.org/licenses/{m.group(1).lower()}/{m.group(2)}/{port}"
    return declared.get(label)


def declared_license_iris(values: list[tuple[str, str]], row: dict | None) -> dict[str, str]:
    """label -> the first IRI (declared in the graph, or in the publisher-terms
    row) that classifies as that label."""
    out = {}
    iris = []
    for k, v in values:
        if k == "iri":
            iris.append(v)
        else:
            bare = v.strip().strip("<>").strip()
            iris += [bare] if re.fullmatch(r"https?://\S+", bare) else [
                u.rstrip(".;:") for u in URL_RE.findall(v)
            ]
    if row is not None:
        iris += [l for l in row["license"] if isinstance(l, str)]
    for iri in iris:
        hit = classify_iri(iri)
        if hit and hit[0] not in out:
            out[hit[0]] = iri.strip()
    return out


def verbatim_only(label: str) -> bool:
    """A licence that allows only unaltered copies: CC BY-ND (and BY-NC-ND)
    and the OGC Document Notice.  The W3C Document License is not one: it
    lets anyone prepare derivative works in software that carry its notice."""
    return bool(re.match(r"CC BY(?:-NC)?-ND\b", label)) or label == "OGC Document Notice"


def license_verdict(classes: list[tuple[str, str]]) -> tuple[str, bool]:
    """Overall status of a vocabulary from its classified values."""
    kinds = {c for _, c in classes}
    if not classes:
        return "none", False
    if RESTRICTED in kinds:
        return "restricted", False
    if UNKNOWN in kinds:
        return "unrecognised", False
    if not kinds & {OPEN, NODERIV}:
        return "copyright-only", False
    return "open", True


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("dump")
    ap.add_argument("out")
    ap.add_argument("--allowlist", help="write the redistributable graph IRIs here")
    ap.add_argument("--snapshot-date", default="unknown")
    ap.add_argument("--source-url", default="https://lov.linkeddata.es/lov.nq.gz")
    args = ap.parse_args()

    sha = hashlib.sha256(open(args.dump, "rb").read()).hexdigest()

    # subject -> predicate -> [(kind, value, lang)] for the metadata graph only
    meta = defaultdict(lambda: defaultdict(list))
    graph_quads = defaultdict(int)
    # graph -> [(subject, kind, value)] licence/rights statements in that graph
    graph_licenses = defaultdict(list)
    # graph -> subjects typed as the vocabulary node
    graph_vocab_nodes = defaultdict(set)
    # graph -> subject -> predicate -> [(kind, value, lang)], for the
    # predicates the notices need (NODE_META_PREDICATES)
    graph_node_meta = defaultdict(lambda: defaultdict(lambda: defaultdict(list)))

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
                continue
            pred = p[1:-1]
            if pred in LICENSE_PREDICATES:
                po = parse_object(o)
                if po is not None and po[0] in ("iri", "lit"):
                    graph_licenses[g[1:-1]].append((s.strip("<>"), po[0], po[1]))
            elif p == RDF_TYPE and o[1:-1] in VOCAB_NODE_TYPES:
                graph_vocab_nodes[g[1:-1]].add(s.strip("<>"))
            if pred in NODE_META_PREDICATES:
                po = parse_object(o)
                if po is not None and po[0] in ("iri", "lit"):
                    graph_node_meta[g[1:-1]][s.strip("<>")][pred].append(po)

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

    # The URI and namespace of every catalogued vocabulary, so a licence one
    # vocabulary states about another is not mistaken for its own.
    trim = lambda x: x.rstrip("#/")
    vocab_nodes = set()
    for s in vocab_subjects:
        vocab_nodes.add(trim(s[1:-1]))
        nsp = first(s, VANN + "preferredNamespaceUri", "lit") or first(
            s, VANN + "preferredNamespaceUri", "iri"
        )
        if nsp:
            vocab_nodes.add(trim(nsp))

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

    def declared_license(uri, nsp):
        """Licence statements about the vocabulary, from its own graph.

        Statements on the vocabulary node (its URI or namespace, or a subject
        typed owl:Ontology / voaf:Vocabulary / skos:ConceptScheme that is not
        another catalogued vocabulary) win.  When
        that node names no licence — nothing, or only a copyright line — the
        other statements in the graph count too (vocab.org's files, for one,
        put the licence on the document node and the copyright line on the
        ontology), except those about another catalogued vocabulary.
        """
        stmts = graph_licenses.get(uri, [])
        node_ids = {trim(uri), trim(nsp or uri)}
        typed = graph_vocab_nodes.get(uri, set())
        on_node = [
            (k, v)
            for s, k, v in stmts
            if trim(s) in node_ids or (s in typed and trim(s) not in vocab_nodes)
        ]
        names_licence = any(
            c != NOTICE for k, v in on_node for _, c in classify_value(k, v)
        )
        chosen = on_node
        if not names_licence:
            chosen = on_node + [
                (k, v)
                for s, k, v in stmts
                if trim(s) not in node_ids and trim(s) not in vocab_nodes
            ]
        seen, out = set(), []
        for k, v in chosen:
            key = (k, norm_ws(v))
            if key not in seen and norm_ws(v):
                seen.add(key)
                out.append((k, v.strip()))
        return out

    def node_meta(uri, nsp):
        """What the graph says about its vocabulary node (its URI or
        namespace), then about each other ontology node it defines (typed
        owl:Ontology and so on, not another catalogued vocabulary), for
        NODE_META_PREDICATES: [vocabulary node, other nodes...]."""
        node_ids = {trim(uri), trim(nsp or uri)}
        typed = graph_vocab_nodes.get(uri, set())
        primary, others = defaultdict(list), []
        for s, preds in graph_node_meta.get(uri, {}).items():
            if trim(s) in node_ids:
                for p, vals in preds.items():
                    primary[p].extend(vals)
            elif s in typed and trim(s) not in vocab_nodes:
                others.append(preds)
        return [primary] + others

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

        declared = declared_license(uri, nsp)
        classes = [c for k, v in declared for c in classify_value(k, v)]
        status, redistributable = license_verdict(classes)
        source, source_url, notice, terms_row = "graph", None, None, None
        if status in ("none", "copyright-only"):
            # The graph names no licence: use the terms its publisher states
            # elsewhere, when we have them on record.
            row = publisher_terms(prefix, uri, nsp)
            if row is not None:
                classes = classes + _terms_classes(row)
                status, redistributable = license_verdict(classes)
                source, source_url, notice = "publisher-terms", row["url"], row["notice"]
                terms_row = row
        labels = []
        for label, cls in classes:
            if cls in (OPEN, NODERIV, RESTRICTED) and label not in labels:
                labels.append(label)
        label_iris = declared_license_iris(declared, terms_row)
        uris = [license_uri(label, label_iris) for label in labels]
        # Every licence offered allows only unaltered copies.
        no_derivatives = any(verbatim_only(l) for l in labels) and not any(
            c == OPEN for _, c in classes
        )

        # The notice copies must carry (see GRAPH_NOTICES and
        # w3c_document_notice); a copyright line the graph states itself is
        # the "pre-existing" notice the document licences ask to keep.
        own_copyright = [
            v
            for k, v in declared
            if COPYRIGHT_RE.search(v) and all(c == NOTICE for _, c in classify_value(k, v))
        ]
        w3c_doc = next(
            (m.group(1) for m in (re.fullmatch(r"W3C Document License \((\d{4})\)", l) for l in labels) if m),
            None,
        )
        if uri in GRAPH_NOTICES:
            notice = GRAPH_NOTICES[uri]
        elif w3c_doc:
            lov_title = next((t for t, _ in values(subj, DCT + "title", "lit")), prefix)
            notice = w3c_document_notice(
                w3c_doc, node_meta(uri, nsp), versions, uri, lov_title, own_copyright
            )
        elif notice == OGC_DOCUMENT_NOTICE:
            notice = (" ".join(own_copyright) + " " if own_copyright else "") + OGC_FALLBACK.format(uri=uri)
        if notice in (W3C_DOCUMENT_NOTICE, OGC_DOCUMENT_NOTICE):
            raise ValueError(f"no notice built for {uri}")
        if uri in EXTRA_NOTICES:
            notice = (notice + " " if notice else "") + EXTRA_NOTICES[uri]

        # Withheld although the licence would allow redistribution: LOV's
        # copy is not faithful (WITHHELD), or the licence requires a copyright
        # notice nobody states.
        withheld = WITHHELD.get(uri) if redistributable else None
        open_labels = list(dict.fromkeys(l for l, c in classes if c in (OPEN, NODERIV)))
        if (
            redistributable
            and not withheld
            and open_labels
            and set(open_labels) <= COPYRIGHT_LINE_LICENCES
            and not own_copyright
            and not (notice and COPYRIGHT_RE.search(notice))
        ):
            withheld = (
                f"{', '.join(open_labels)} requires the copyright notice on every copy, and neither "
                "the vocabulary nor its rights holder states one."
            )
        if withheld:
            redistributable = False

        vocabularies.append(
            {
                "prefix": prefix,
                "uri": uri,
                "nsp": nsp or uri,
                "titles": lang_strings(subj, DCT + "title"),
                "descriptions": lang_strings(subj, DCT + "description") if redistributable else [],
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
                "license": labels,
                # The URI of each licence in `license`, in the same order
                # (LICENSE_URIS; null where none is known).
                "license_uris": uris,
                "license_declared": [v for _, v in declared],
                "license_status": status,
                "redistributable": redistributable,
                # Every licence offered allows only unaltered copies (CC
                # BY-ND, the OGC Document Notice; see verbatim_only).
                "no_derivatives": no_derivatives,
                # "graph": the licence fields come from the vocabulary's own
                # graph; "publisher-terms": the graph names none and the
                # licence is what its publisher states elsewhere
                # (license_source_url, see PUBLISHER_TERMS).
                "license_source": source,
                "license_source_url": source_url,
                # The statement the licence requires on copies (see
                # GRAPH_NOTICES, PUBLISHER_TERMS and w3c_document_notice).
                "license_notice": notice,
                # Why a vocabulary whose licence would allow redistribution
                # is not redistributed; None otherwise.
                "redistribution_withheld": withheld,
                # Literals of LOV's copy that hold mis-decoded characters, in a
                # graph that is shipped (see mojibake(); 0 otherwise).  The
                # notice then says so.
                "lov_misdecoded": 0,
                "_noderiv": any(c == NODERIV for _, c in classes),
            }
        )

    # Text LOV mis-decoded (UTF-8 read as Latin-1 or Windows-1252; see
    # mojibake()), in every graph that would ship; UPSTREAM_MOJIBAKE lists the
    # publishers' own.  Under a licence that forbids modification (CC BY-ND,
    # the W3C and OGC document licences) the image ships LOV's copy only when
    # it is faithful, so such a graph is withheld.  Under any other licence
    # the graph ships as LOV holds it and its notice says it has mis-decoded
    # text (lov_misdecoded counts the literals).  For the no-modification
    # licences a description is also kept only when it is character for
    # character a literal of the vocabulary's own graph (LOV may have
    # shortened, reworded or re-spaced it).
    noderiv = {v["uri"]: v for v in vocabularies if v["_noderiv"] and v["redistributable"]}
    shipped_now = {v["uri"]: v for v in vocabularies if v["redistributable"] and v["graph_quads"] > 0}
    wanted = {f"<{u}>" for u in (set(noderiv) | set(shipped_now))}
    literals = defaultdict(set)
    misdecoded = defaultdict(int)
    with gzip.open(args.dump, "rt", encoding="utf-8", errors="replace") as f:
        for line in f:
            m = LINE_RE.match(line)
            if not m or m.group(4) not in wanted:
                continue
            po = parse_object(m.group(3))
            if po is not None and po[0] == "lit":
                g = m.group(4)[1:-1]
                if g in noderiv:
                    literals[g].add(po[1])
                if mojibake(po[1]):
                    misdecoded[g] += 1
    for uri in sorted(UPSTREAM_MOJIBAKE - set(misdecoded)):
        print(f"note: UPSTREAM_MOJIBAKE entry {uri} no longer applies", file=sys.stderr)
    for uri in sorted(set(LOV_MISDECODING_CHECKED) - set(misdecoded)):
        print(f"note: LOV_MISDECODING_CHECKED entry {uri} no longer applies", file=sys.stderr)
    lov_errors = {u: n for u, n in misdecoded.items() if u not in UPSTREAM_MOJIBAKE}
    for uri, v in noderiv.items():
        if uri in lov_errors:
            v["redistributable"] = False
            v["redistribution_withheld"] = (
                "LOV's copy has text that was mis-decoded, so it is not a faithful copy, and "
                f"{', '.join(v['license'])} allows no modified copies."
            )
            continue
        v["descriptions"] = [d for d in v["descriptions"] if d["value"] in literals.get(uri, ())]
    for uri, v in shipped_now.items():
        n = lov_errors.get(uri, 0)
        if not n or not v["redistributable"]:
            continue
        literal_s, hold = ("literal", "holds") if n == 1 else ("literals", "hold")
        if uri in LOV_MISDECODING_CHECKED:
            defect = (
                f"LOV mis-decoded characters in {n} {literal_s} of its copy of this vocabulary "
                "(UTF-8 read as Latin-1 or Windows-1252), which ships as LOV distributes it: in "
                "those literals it differs from the publisher's document."
            )
        else:
            defect = (
                f"{n} {literal_s} of LOV's copy of this vocabulary, which ships as LOV distributes "
                f"it, {hold} mis-decoded characters (UTF-8 read as Latin-1 or Windows-1252); where "
                "the publisher's document has the correct characters, this copy differs from it "
                "in those literals."
            )
        v["lov_misdecoded"] = n
        before = v["license_notice"] or ""
        if before and not before.endswith("."):
            before += "."
        v["license_notice"] = (before + " " if before else "") + defect
    for v in vocabularies:
        if not v["redistributable"]:
            v["descriptions"] = []
        del v["_noderiv"]

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
            # LOV's own licence, for LOV's metadata only: each vocabulary's
            # licence is in its entry.
            "license": LOV_LICENSE,
            "license_url": LOV_LICENSE_URL,
            "license_scope": (
                "LOV's own catalogue metadata. Each vocabulary, and any text quoted from it, "
                "stays under its publisher's licence, given per entry."
            ),
            "attribution": "Linked Open Vocabularies (LOV), https://lov.linkeddata.es/",
            "modifications": (
                "Extracted from the LOV metadata graph of this dump and converted to JSON by "
                "scripts/build_lov_catalog.py. Descriptions are dropped for vocabularies that "
                "are not redistributed. The licence fields (license, license_uris, "
                "license_declared, license_status, redistributable, no_derivatives, "
                "license_source, license_source_url, license_notice, redistribution_withheld, "
                "lov_misdecoded: from each vocabulary's own graph or, where it states none, its "
                "publisher's published terms), incoming_links, graph_quads and term_metrics are "
                "computed by Open Triplestore."
            ),
        },
        "vocabularies": vocabularies,
        "term_metrics": term_metrics,
    }

    payload = json.dumps(catalog, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    # mtime=0 keeps the output byte-identical across rebuilds of the same dump.
    with open(args.out, "wb") as raw, gzip.GzipFile(
        filename="", mode="wb", fileobj=raw, compresslevel=9, mtime=0
    ) as f:
        f.write(payload)

    shipped = [v for v in vocabularies if v["redistributable"] and v["graph_quads"] > 0]
    for v in shipped:
        unknown = [
            l for l, u in zip(v["license"], v["license_uris"]) if not u and l not in NO_LICENSE_URI
        ]
        if unknown:
            raise ValueError(f"no licence URI for {unknown} of the shipped graph {v['uri']}")
    if args.allowlist:
        with open(args.allowlist, "w", encoding="utf-8") as f:
            f.write(
                "# Vocabulary graphs of the LOV dump that the Docker image may ship.\n"
                "# Generated by scripts/build_lov_catalog.py; do not edit by hand.\n"
                f"# Dump: {args.source_url} (snapshot {args.snapshot_date}, sha256 {sha})\n"
                "# A graph is listed only when its licence allows redistributing a copy:\n"
                "# the licence the vocabulary declares in its own graph or, where the\n"
                "# graph names none, the terms its publisher states elsewhere (see the\n"
                "# script's PUBLISHER_TERMS), and when LOV's copy can be shipped under it\n"
                "# (see WITHHELD).  Each graph is LOV's N-Quads serialization as found in\n"
                "# LOV's dump, shipped unchanged; where LOV's copy has mis-decoded text\n"
                "# (UTF-8 read as Latin-1 or Windows-1252) the notice column says so.\n"
                "# Columns (tab-separated): graph IRI, LOV prefix, licence, source of the\n"
                "# licence (\"graph\", or the URL of the publisher's terms), and the notice\n"
                "# the licence requires on copies (empty when it asks only for the usual\n"
                "# attribution).  The catalogue entry's license_uris gives each licence's\n"
                "# URI.\n"
            )
            for v in sorted(shipped, key=lambda v: v["uri"]):
                source = v["license_source_url"] or v["license_source"]
                notice = v["license_notice"] or ""
                for field in (v["uri"], v["prefix"], source, notice, *v["license"]):
                    if "\t" in field or "\n" in field:
                        raise ValueError(f"tab or newline in the allowlist row of {v['uri']}")
                f.write(
                    f"{v['uri']}\t{v['prefix']}\t{'; '.join(v['license'])}\t{source}\t{notice}\n"
                )

    counts = defaultdict(int)
    for v in vocabularies:
        counts[v["license_status"]] += 1
    by_terms = defaultdict(int)
    for v in vocabularies:
        if v["license_source"] == "publisher-terms":
            by_terms[v["license_status"]] += 1
    print(f"vocabularies: {len(vocabularies)}")
    print(f"licence status: {dict(sorted(counts.items()))}")
    print(f"from publisher terms: {dict(sorted(by_terms.items()))}")
    print(f"redistributable graphs in the corpus: {len(shipped)}")
    withheld = [v["prefix"] for v in vocabularies if v["redistribution_withheld"]]
    print(f"withheld although the licence would allow it: {len(withheld)} {withheld}")
    print(f"with a required notice: {sum(1 for v in shipped if v['license_notice'])}")
    print(f"no derivatives: {[v['prefix'] for v in shipped if v['no_derivatives']]}")
    print(f"mis-decoded text: {[(v['prefix'], v['lov_misdecoded']) for v in shipped if v['lov_misdecoded']]}")
    print(f"descriptions kept: {sum(1 for v in vocabularies if v['descriptions'])}")
    print(f"term metrics: {len(term_metrics)}")
    print(f"raw json: {len(payload)/1e6:.2f} MB, gz: {__import__('os').path.getsize(args.out)/1e6:.2f} MB")
    return 0


if __name__ == "__main__":
    sys.exit(main())
