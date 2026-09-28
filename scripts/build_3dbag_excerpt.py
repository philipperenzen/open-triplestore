#!/usr/bin/env python3
"""Build the bundled 3DBAG CityJSON excerpt around the Schependomlaan site.

Fetches every BAG pand inside BBOX from the 3DBAG OGC Features API
(CityJSONFeature sequence), merges them into one CityJSON 2.0 document, and
writes frontend/public/samples/schependomlaan-3dbag.city.json.

Kept per building (the rest is dropped to keep the bundled file lean):
  * the Building object's LoD0 footprint + its curated BAG/3DBAG attributes
    (year built, status, roof type/heights, volume, areas, quality indicator);
  * the BuildingPart's LoD2.2 Solid with its semantic surfaces (roof/wall/
    ground), which the client-side viewer colours;
  * the parent/child topology between the two.

Vertices are re-indexed so dropped LoD1.2/1.3 solids leave no dead weight.

The LoD0 footprint vertices are LIFTED from z=0 to the building's ground level
(b3_h_maaiveld, NAP): 3DBAG stores footprints at absolute zero while the LoD2.2
solids carry real NAP heights (~10 m here), and any viewer that grounds a model
at its lowest vertex would otherwise float every solid a storey above the map.

Data: © 3DBAG by tudelft3d and 3DGI, CC BY 4.0
(https://creativecommons.org/licenses/by/4.0/), https://docs.3dbag.nl/en/copyright/.
The output is an adaptation, so its metadata.title says "modified", names the
changes above and carries the licence URI (CC BY 4.0 §3(a)(1)).

Usage:  python scripts/build_3dbag_excerpt.py
"""

import json
import sys
import urllib.request
from pathlib import Path

API = "https://api.3dbag.nl/collections/pand/items"
# 150 m x 150 m of RD (EPSG:7415) around the Schependomlaan street in Nijmegen
# (street centroid RD 185772,428156 per PDOK locatieserver, at the NE corner).
BBOX = "185610,428010,185760,428160"
OUT = Path(__file__).resolve().parent.parent / "frontend/public/samples/schependomlaan-3dbag.city.json"

# The 3DBAG release the API serves, recorded in the output. Check it against
# https://docs.3dbag.nl/en/overview/release_notes/ on every regeneration: the
# API's own collection metadata (version.collection) still reported v2023.10.08
# while serving 2025.09.03 data (b3_h_nok, first published in 2025.09.03).
RELEASE = "2025.09.03"

# Attribution + licence notice. The credit wording is 3DBAG's own
# (https://docs.3dbag.nl/en/copyright/); CC BY 4.0 §3(a)(1) adds the licence
# URI and the statement that this copy was modified.
CREDIT = "© 3DBAG by tudelft3d and 3DGI"
LICENSE_URI = "https://creativecommons.org/licenses/by/4.0/"
COPYRIGHT_PAGE = "https://docs.3dbag.nl/en/copyright/"

# Curated attribute allow-list: the BAG registry facts + the 3DBAG-derived
# building metrics that make good linked data. Percentile/nodata/mutation
# bookkeeping columns are dropped.
KEEP_ATTRS = [
    "identificatie",
    "oorspronkelijkbouwjaar",
    "status",
    "b3_dak_type",
    "b3_bouwlagen",
    "b3_h_maaiveld",
    "b3_h_dak_min",
    "b3_h_dak_50p",
    "b3_h_dak_max",
    "b3_h_nok",
    "b3_volume_lod22",
    "b3_opp_grond",
    "b3_opp_dak_plat",
    "b3_opp_dak_schuin",
    "b3_opp_buitenmuur",
    "b3_kwaliteitsindicator",
    "b3_azimut",
    "b3_hellingshoek",
]


def fetch_all():
    feats, url = [], f"{API}?bbox={BBOX}&limit=100"
    while url:
        print(f"fetch {url}", file=sys.stderr)
        with urllib.request.urlopen(url, timeout=120) as r:
            page = json.load(r)
        feats.extend(page.get("features", []))
        url = next((l["href"] for l in page.get("links", []) if l.get("rel") == "next"), None)
        skeleton = page.get("metadata") or {}
    return skeleton, feats


def keep_geometry(obj_type, geom):
    lod = str(geom.get("lod"))
    if obj_type == "Building":
        return lod == "0"
    return lod == "2.2"


def remap_boundaries(b, mapping):
    if isinstance(b, int):
        return mapping[b]
    return [remap_boundaries(x, mapping) for x in b]


def round3(v):
    if isinstance(v, float):
        return round(v, 3)
    return v


def main():
    skeleton, feats = fetch_all()
    transform = skeleton["transform"]
    ref = skeleton.get("metadata", {}).get("referenceSystem", "https://www.opengis.net/def/crs/EPSG/0/7415")

    out_objects, out_vertices = {}, []
    n_buildings = n_parts = 0
    for f in feats:
        f_vertices = f.get("vertices", [])
        mapping = {}

        def vidx(i, z_override=None):
            key = (i, z_override)
            if key not in mapping:
                mapping[key] = len(out_vertices)
                v = list(f_vertices[i])
                if z_override is not None:
                    v[2] = z_override
                out_vertices.append(v)
            return mapping[key]

        for oid, obj in f.get("CityObjects", {}).items():
            otype = obj.get("type")
            # Ground level for this feature, in quantised z units — used to lift
            # the LoD0 footprint out of the z=0 plane (see module docstring).
            maaiveld = None
            for o2 in f.get("CityObjects", {}).values():
                a2 = o2.get("attributes") or {}
                if a2.get("b3_h_maaiveld") is not None:
                    maaiveld = round((a2["b3_h_maaiveld"] - transform["translate"][2]) / transform["scale"][2])
                    break
            kept_geoms = []
            for g in obj.get("geometry", []) or []:
                if not keep_geometry(otype, g):
                    continue
                is_lod0 = str(g.get("lod")) == "0"
                g = dict(g)
                g["boundaries"] = remap_boundaries(
                    g["boundaries"], _IndexMapper(lambda i: vidx(i, maaiveld if is_lod0 else None))
                )
                g.pop("texture", None)
                g.pop("material", None)
                kept_geoms.append(g)
            slim = {"type": otype}
            if kept_geoms:
                slim["geometry"] = kept_geoms
            if otype == "Building":
                attrs = obj.get("attributes") or {}
                slim["attributes"] = {k: round3(attrs[k]) for k in KEEP_ATTRS if attrs.get(k) is not None}
                n_buildings += 1
            else:
                n_parts += 1
            if obj.get("parents"):
                slim["parents"] = obj["parents"]
            if obj.get("children"):
                slim["children"] = obj["children"]
            out_objects[oid] = slim

    doc = {
        "type": "CityJSON",
        "version": "2.0",
        "metadata": {
            "title": (
                f"Schependomlaan block, Nijmegen - 3DBAG LoD2.2 excerpt, modified from 3DBAG {RELEASE} "
                "(api.3dbag.nl): LoD1.2/1.3 geometry dropped, building attributes cut to an allow-list and "
                "rounded to 3 decimals, LoD0 footprints raised from z=0 to ground level (b3_h_maaiveld), "
                "merged into one CityJSON 2.0 document. "
                f"{CREDIT}, CC BY 4.0 {LICENSE_URI} - {COPYRIGHT_PAGE}"
            ),
            "referenceSystem": ref,
        },
        "transform": transform,
        "CityObjects": out_objects,
        "vertices": out_vertices,
    }
    # ensure_ascii=False writes the credit's "©" as-is, not as a "\u00a9" escape.
    OUT.write_text(json.dumps(doc, separators=(",", ":"), ensure_ascii=False), encoding="utf-8")
    print(
        f"wrote {OUT} - {n_buildings} buildings / {n_parts} parts, "
        f"{len(out_vertices)} vertices, {OUT.stat().st_size / 1024:.0f} KiB",
        file=sys.stderr,
    )


class _IndexMapper:
    """Lets remap_boundaries treat the closure like a mapping."""

    def __init__(self, fn):
        self.fn = fn

    def __getitem__(self, i):
        return self.fn(i)


if __name__ == "__main__":
    main()
