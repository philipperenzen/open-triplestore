#!/usr/bin/env python3
"""Generate per-dataset brand icons for the bundled demo datasets.

Each seeded demo dataset (one per standards category) gets a 512x512 PNG icon:
the same knowledge-graph mark as the org logo, re-themed to the category's
accent so the icon and its animated banner preset
(`frontend/src/lib/banners.ts`) share a hue. Dependency-free — it reuses the
stdlib-only PNG/SDF primitives in `gen_org_brand.py` (the image-upload
allow-list rejects SVG, so the seed needs raster assets).

Outputs (committed, then bundled into the binary via `include_bytes!` in
`src/saved_queries/seed.rs`):
    docs/assets/dataset-brand/<slug>.png   512x512

Run:  python scripts/gen_dataset_brand.py
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import gen_org_brand as g  # noqa: E402  reuse the dependency-free primitives


def _hex_to_rgb(c):
    c = c.lstrip("#")
    return (int(c[0:2], 16), int(c[2:4], 16), int(c[4:6], 16))


def _rgb_to_hex(r, gr, b):
    clamp = lambda v: max(0, min(255, int(v + 0.5)))
    return "#%02x%02x%02x" % (clamp(r), clamp(gr), clamp(b))


def _toward(c, target, t):
    """Mix hex colour `c` toward hex `target` by fraction t in [0, 1]."""
    r0, g0, b0 = _hex_to_rgb(c)
    r1, g1, b1 = _hex_to_rgb(target)
    return _rgb_to_hex(r0 + (r1 - r0) * t, g0 + (g1 - g0) * t, b0 + (b1 - b0) * t)


WHITE = "#ffffff"
BLACK = "#000000"

# slug -> (dark tile end, bright tile end, accent), aligned with the matching
# animated banner preset in frontend/src/lib/banners.ts so a dataset's icon and
# its banner share a hue.
THEMES = {
    "core-rdf-sparql": ("#0f2a33", "#2f7a8c", "#7ed6d0"),  # teal
    "reasoning":       ("#332a17", "#9c7a3f", "#f0c478"),  # amber
    "spatial":         ("#0e2b25", "#2f8c6e", "#78dcaa"),  # emerald
    "validation":      ("#1f1733", "#6a4f9c", "#c296f5"),  # violet
    "rules":           ("#0e2336", "#2f6aa0", "#7daffa"),  # azure
    "linked-data":     ("#0f3033", "#2f8c8c", "#78dceb"),  # cyan
    "capabilities":    ("#331720", "#9c4f63", "#f58cac"),  # rose
    "ots-ontology":    ("#171c33", "#4f5a9c", "#9ba2f5"),  # indigo
}


def _apply_theme(dark, bright, accent):
    """Override gen_org_brand's module-global palette for one theme."""
    g.TILE_TL = _toward(dark, bright, 0.25)
    g.TILE_BR = _toward(dark, BLACK, 0.35)
    g.GLOW = accent
    g.RING_0 = _toward(accent, WHITE, 0.55)
    g.RING_1 = accent
    g.RING_2 = _toward(accent, BLACK, 0.22)
    g.NODE_0 = _toward(accent, WHITE, 0.70)
    g.NODE_1 = accent
    g.NODE_2 = bright
    g.NODE_RING = _toward(accent, WHITE, 0.85)
    g.EDGE = _toward(accent, WHITE, 0.55)


def main():
    out = os.path.normpath(
        os.path.join(os.path.dirname(__file__), "..", "docs", "assets", "dataset-brand")
    )
    os.makedirs(out, exist_ok=True)
    for slug, (dark, bright, accent) in THEMES.items():
        _apply_theme(dark, bright, accent)
        g.write_png(os.path.join(out, f"{slug}.png"), g.render_logo(512))


if __name__ == "__main__":
    main()
