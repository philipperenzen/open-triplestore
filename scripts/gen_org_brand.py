#!/usr/bin/env python3
"""Generate the Open Triplestore demo-organisation branding (logo + banner).

The brand mark is a teal "O" ring crossed by three knowledge-graph nodes joined
in a triangle — the same motif as docs/assets/logo.svg and the startup banner.
The image-upload allow-list rejects SVG (stored-XSS hardening), so the seed needs
*raster* assets. This script renders them as PNGs with no third-party
dependencies (stdlib zlib only), using analytic signed-distance fields with 1px
anti-aliasing so the curves stay crisp.

Outputs (committed, then bundled into the binary via include_bytes! in
src/saved_queries/seed.rs):
    docs/assets/org-logo.png     512x512  — square avatar/logo
    docs/assets/org-banner.png   1600x400 — wide page header

Run:  python scripts/gen_org_brand.py
"""

import math
import os
import struct
import zlib

# ── Brand palette (from frontend/src/theme.css + docs/assets/logo.svg) ────────
TILE_TL   = "#1f5563"   # tile gradient, top-left
TILE_BR   = "#0b262e"   # tile gradient, bottom-right (a touch darker for depth)
GLOW      = "#7ED6D0"   # teal glow / accent
RING_0    = "#cdf6f1"   # ring gradient stops
RING_1    = "#7ED6D0"
RING_2    = "#56b6bd"
NODE_0    = "#d4f7f2"   # node radial gradient stops
NODE_1    = "#6fcdc9"
NODE_2    = "#2F7A8C"
NODE_RING = "#eafdfb"   # node outline
EDGE      = "#dbf7f3"   # graph edges

AA = 1.0  # anti-alias width in pixels


# ── colour helpers ────────────────────────────────────────────────────────────
def hx(c):
    c = c.lstrip("#")
    return (int(c[0:2], 16) / 255.0, int(c[2:4], 16) / 255.0, int(c[4:6], 16) / 255.0)


def mix(a, b, t):
    return (a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t)


def grad(stops, t):
    """Piecewise-linear colour ramp. stops = [(offset, (r,g,b)), ...]."""
    t = 0.0 if t < 0 else 1.0 if t > 1 else t
    for i in range(len(stops) - 1):
        o0, c0 = stops[i]
        o1, c1 = stops[i + 1]
        if t <= o1:
            span = o1 - o0
            return c0 if span <= 0 else mix(c0, c1, (t - o0) / span)
    return stops[-1][1]


def clamp01(x):
    return 0.0 if x < 0 else 1.0 if x > 1 else x


# ── signed-distance fields (negative = inside) ────────────────────────────────
def sd_disc(px, py, cx, cy, r):
    return math.hypot(px - cx, py - cy) - r


def sd_ring(px, py, cx, cy, radius, half_t):
    return abs(math.hypot(px - cx, py - cy) - radius) - half_t


def sd_segment(px, py, ax, ay, bx, by, half_w):
    vx, vy = bx - ax, by - ay
    wx, wy = px - ax, py - ay
    den = vx * vx + vy * vy
    t = 0.0 if den == 0 else (wx * vx + wy * vy) / den
    t = clamp01(t)
    return math.hypot(px - (ax + vx * t), py - (ay + vy * t)) - half_w


# ── canvas (straight-alpha RGBA floats) ───────────────────────────────────────
class Canvas:
    def __init__(self, w, h):
        self.w, self.h = w, h
        self.buf = [0.0] * (w * h * 4)

    def blend(self, x, y, rgb, a):
        if a <= 0:
            return
        i = (y * self.w + x) * 4
        b = self.buf
        da = b[i + 3]
        out_a = a + da * (1 - a)
        if out_a <= 0:
            return
        for k in range(3):
            b[i + k] = (rgb[k] * a + b[i + k] * da * (1 - a)) / out_a
        b[i + 3] = out_a

    def fill(self, color_fn):
        """Paint every pixel opaque from color_fn(x, y) -> rgb (the base layer)."""
        b = self.buf
        for y in range(self.h):
            row = y * self.w
            for x in range(self.w):
                i = (row + x) * 4
                r, g, bl = color_fn(x + 0.5, y + 0.5)
                b[i], b[i + 1], b[i + 2], b[i + 3] = r, g, bl, 1.0

    def shape(self, bbox, sdf, color_fn, alpha=1.0):
        """Composite an SDF shape, AA'd, but only across its bounding box."""
        x0, y0, x1, y1 = bbox
        x0 = max(0, int(math.floor(x0)));  y0 = max(0, int(math.floor(y0)))
        x1 = min(self.w, int(math.ceil(x1))); y1 = min(self.h, int(math.ceil(y1)))
        for y in range(y0, y1):
            py = y + 0.5
            for x in range(x0, x1):
                px = x + 0.5
                cov = clamp01(0.5 - sdf(px, py) / AA)
                if cov > 0:
                    self.blend(x, y, color_fn(px, py), cov * alpha)

    def to_png_bytes(self):
        w, h, b = self.w, self.h, self.buf
        raw = bytearray()
        stride = w * 4
        for y in range(h):
            raw.append(0)  # filter type 0 (None)
            base = y * stride
            for v in b[base:base + stride]:
                iv = int(v * 255 + 0.5)
                raw.append(0 if iv < 0 else 255 if iv > 255 else iv)
        comp = zlib.compress(bytes(raw), 9)

        def chunk(typ, data):
            return (struct.pack(">I", len(data)) + typ + data +
                    struct.pack(">I", zlib.crc32(typ + data) & 0xffffffff))

        ihdr = struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)  # 8-bit RGBA
        return (b"\x89PNG\r\n\x1a\n" +
                chunk(b"IHDR", ihdr) + chunk(b"IDAT", comp) + chunk(b"IEND", b""))


def write_png(path, canvas):
    with open(path, "wb") as f:
        f.write(canvas.to_png_bytes())
    print(f"wrote {path} ({canvas.w}x{canvas.h})")


# ── the brand mark: O-ring + three nodes + connecting edges ───────────────────
def draw_mark(cv, cx, cy, R, *, ring_w, node_r, edge_w, ring_a=1.0, node_a=1.0,
              edge_a=1.0, glow_a=0.0):
    """Draw the knowledge-graph mark centred at (cx, cy) with ring radius R.

    Geometry mirrors docs/assets/logo.svg: nodes sit on the ring at the bottom
    and the two upper corners (an inverted triangle), joined by straight edges.
    """
    # Node centres on the ring (angles: bottom, upper-right, upper-left).
    ang = [math.pi / 2, math.pi / 2 + 2 * math.pi / 3, math.pi / 2 - 2 * math.pi / 3]
    nodes = [(cx + R * math.cos(a), cy + R * math.sin(a)) for a in ang]

    edge_rgb = hx(EDGE)
    ring_stops = [(0.0, hx(RING_0)), (0.48, hx(RING_1)), (1.0, hx(RING_2))]
    half_t = ring_w / 2

    # Ring gradient runs diagonally (top-left -> bottom-right), as in the SVG.
    gx0, gy0 = cx - R, cy - R
    glen = 2 * R * math.sqrt(2)

    def ring_col(px, py):
        t = ((px - gx0) + (py - gy0)) / (glen if glen else 1)
        return grad(ring_stops, t)

    # 1) connecting edges (behind the ring)
    for i in range(3):
        ax, ay = nodes[i]
        bx, by = nodes[(i + 1) % 3]
        bb = (min(ax, bx) - edge_w, min(ay, by) - edge_w,
              max(ax, bx) + edge_w, max(ay, by) + edge_w)
        cv.shape(bb, (lambda ax, ay, bx, by: lambda px, py:
                      sd_segment(px, py, ax, ay, bx, by, edge_w / 2))(ax, ay, bx, by),
                 lambda px, py: edge_rgb, alpha=edge_a)

    # 2) the "O" ring
    m = half_t + AA
    cv.shape((cx - R - m, cy - R - m, cx + R + m, cy + R + m),
             lambda px, py: sd_ring(px, py, cx, cy, R, half_t), ring_col, alpha=ring_a)

    # 3) nodes: optional soft glow, bright outline, then radial-gradient fill
    node_stops = [(0.0, hx(NODE_0)), (0.45, hx(NODE_1)), (1.0, hx(NODE_2))]
    ring_rgb = hx(NODE_RING)
    for (nx, ny) in nodes:
        if glow_a > 0:
            soft_glow(cv, nx, ny, node_r * 2.4, hx(GLOW), glow_a)
        # outline (stroke centred on node edge)
        outer = node_r + edge_w / 2
        cv.shape((nx - outer - AA, ny - outer - AA, nx + outer + AA, ny + outer + AA),
                 (lambda nx, ny, outer: lambda px, py: sd_disc(px, py, nx, ny, outer))(nx, ny, outer),
                 lambda px, py: ring_rgb, alpha=node_a)
        # fill
        def node_col(px, py, nx=nx, ny=ny):
            # radial gradient with a light source up-left (matches the SVG)
            gx = nx - node_r * 0.32
            gy = ny - node_r * 0.44
            t = math.hypot(px - gx, py - gy) / (node_r * 1.7)
            return grad(node_stops, t)
        cv.shape((nx - node_r - AA, ny - node_r - AA, nx + node_r + AA, ny + node_r + AA),
                 (lambda nx, ny: lambda px, py: sd_disc(px, py, nx, ny, node_r))(nx, ny),
                 node_col, alpha=node_a)


def soft_glow(cv, cx, cy, radius, rgb, peak_a):
    """Additive-ish radial glow blended over whatever is beneath."""
    x0 = max(0, int(cx - radius)); x1 = min(cv.w, int(cx + radius) + 1)
    y0 = max(0, int(cy - radius)); y1 = min(cv.h, int(cy + radius) + 1)
    for y in range(y0, y1):
        for x in range(x0, x1):
            d = math.hypot(x + 0.5 - cx, y + 0.5 - cy) / radius
            if d < 1:
                cv.blend(x, y, rgb, (1 - d) ** 2 * peak_a)


# ── logo: 512x512 full-bleed avatar ───────────────────────────────────────────
def render_logo(size=512):
    cv = Canvas(size, size)
    tl, br = hx(TILE_TL), hx(TILE_BR)

    def bg(px, py):
        t = ((px / size) + (py / size)) / 2  # diagonal TL->BR
        return mix(tl, br, t)

    cv.fill(bg)
    soft_glow(cv, size * 0.32, size * 0.24, size * 0.62, hx(GLOW), 0.34)

    c = size / 2
    R = size * 0.295
    draw_mark(cv, c, c, R,
              ring_w=size * 0.072, node_r=size * 0.10, edge_w=size * 0.030,
              glow_a=0.18)
    return cv


# ── banner: 1600x400 wide header ──────────────────────────────────────────────
def render_banner(w=1600, h=400):
    # The org page renders the banner full-width at a fixed 160px height with
    # object-fit:cover, and the content column has no max-width. On a wide
    # viewport that crops to a ~160px-tall central horizontal band, so ALL the
    # meaningful art must live near the vertical centre — outer rows are bleed
    # that only appears on narrow screens.
    cv = Canvas(w, h)
    tl, br = hx(TILE_TL), hx(TILE_BR)
    cy = h / 2

    def bg(px, py):
        t = clamp01((px / w) * 0.62 + (py / h) * 0.38)
        return mix(tl, br, t)

    cv.fill(bg)
    soft_glow(cv, w * 0.14, cy * 0.7, h * 1.5, hx(GLOW), 0.22)
    soft_glow(cv, w * 0.88, cy * 1.35, h * 1.5, hx(NODE_2), 0.32)
    soft_glow(cv, w * 0.50, cy, h * 1.7, hx(GLOW), 0.06)  # gentle centre wash

    # Knowledge-graph constellation flowing left -> right into the hero mark.
    # f is a 0..1 vertical position kept inside the safe band (~125..275).
    band = h * 0.38  # total vertical spread of the constellation
    pts = [(80, 0.32), (210, 0.78), (330, 0.22), (450, 0.62), (560, 0.30),
           (690, 0.70), (820, 0.40), (940, 0.74), (1010, 0.30), (1120, 0.58)]
    nodes = [(x, cy + (f - 0.5) * band) for (x, f) in pts]
    edges = [(0, 1), (0, 2), (2, 3), (1, 3), (3, 4), (4, 5), (5, 6), (4, 6),
             (6, 7), (6, 8), (8, 9), (7, 9)]
    edge_rgb = hx(EDGE)
    for (a, b) in edges:
        ax, ay = nodes[a]; bx, by = nodes[b]
        bb = (min(ax, bx) - 2, min(ay, by) - 2, max(ax, bx) + 2, max(ay, by) + 2)
        cv.shape(bb, (lambda ax, ay, bx, by: lambda px, py:
                      sd_segment(px, py, ax, ay, bx, by, 0.9))(ax, ay, bx, by),
                 lambda px, py: edge_rgb, alpha=0.18)
    for (nx, ny) in nodes:
        soft_glow(cv, nx, ny, 17, hx(GLOW), 0.24)
        cv.shape((nx - 6, ny - 6, nx + 6, ny + 6),
                 (lambda nx, ny: lambda px, py: sd_disc(px, py, nx, ny, 3.6))(nx, ny),
                 lambda px, py: hx(NODE_0), alpha=0.60)

    # Hero brand mark, sized to sit fully inside the safe central band.
    draw_mark(cv, w * 0.80, cy, h * 0.155,
              ring_w=11, node_r=11, edge_w=4,
              ring_a=0.9, node_a=1.0, edge_a=0.55, glow_a=0.32)
    return cv


def main():
    out = os.path.join(os.path.dirname(__file__), "..", "docs", "assets")
    out = os.path.normpath(out)
    os.makedirs(out, exist_ok=True)
    write_png(os.path.join(out, "org-logo.png"), render_logo())
    write_png(os.path.join(out, "org-banner.png"), render_banner())


if __name__ == "__main__":
    main()
