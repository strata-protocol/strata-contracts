#!/usr/bin/env python3
"""
Render the Strata logo.

Emits assets/logo.svg (the reviewable source) and assets/logo.png (a raster of
exactly the same geometry, for anywhere SVG is not accepted).

The mark is three stacked bars of decreasing width: geological strata, which is
also the senior/junior split. The brightest bar is on top, because the senior
tranche is paid first and capped at its target.

Run from the repository root:

    python assets/generate-logo.py

No third-party dependencies. The PNG is written by hand: signature, IHDR, a
zlib-compressed IDAT of RGBA scanlines, IEND. Edges are anti-aliased by 4x4
supersampling, which is why the rounded ends stay smooth at 32px.
"""

import struct
import zlib
from pathlib import Path

# --- design ------------------------------------------------------------------
# One source of truth for both outputs. The SVG writer and the rasteriser both
# read these, so the two files cannot drift apart.

SIZE = 512
BG = (0x0D, 0x0B, 0x1A, 0xFF)  # near-black indigo
CORNER = 104  # background corner radius

# (width, colour) top to bottom. Decreasing width upwards.
BARS = [
    (168, (0xB9, 0x8C, 0xFF, 0xFF)),  # bright violet - senior, capped
    (264, (0x7D, 0x00, 0xFF, 0xFF)),  # Stellar purple - the vault
    (360, (0x46, 0x1B, 0x9E, 0xFF)),  # deep violet - junior, takes the loss
]

BAR_H = 54
BAR_GAP = 38
BAR_R = BAR_H // 2  # fully rounded ends


def bar_boxes():
    """Top-left x, y and width of each bar, vertically centred as a group."""
    total = len(BARS) * BAR_H + (len(BARS) - 1) * BAR_GAP
    y = (SIZE - total) / 2
    out = []
    for w, _colour in BARS:
        out.append(((SIZE - w) / 2, y, w))
        y += BAR_H + BAR_GAP
    return out


# --- geometry ----------------------------------------------------------------


def rounded_rect_contains(px, py, x, y, w, h, r):
    """Point-in-rounded-rectangle, inclusive of the boundary."""
    if px < x or px > x + w or py < y or py > y + h:
        return False
    # Distance into the nearest corner box.
    cx = min(max(px, x + r), x + w - r)
    cy = min(max(py, y + r), y + h - r)
    dx, dy = px - cx, py - cy
    return dx * dx + dy * dy <= r * r


def over(dst, src, coverage):
    """Composite src over dst at the given coverage, both non-premultiplied RGBA."""
    sa = src[3] / 255.0 * coverage
    if sa <= 0:
        return dst
    da = dst[3] / 255.0
    out_a = sa + da * (1 - sa)
    if out_a <= 0:
        return (0, 0, 0, 0, 0.0)
    out = []
    for i in range(3):
        sc = src[i] / 255.0
        dc = dst[i] / 255.0
        out.append(round(255 * (sc * sa + dc * da * (1 - sa)) / out_a))
    return (out[0], out[1], out[2], round(out_a * 255), 0.0)


def render_rgba(supersample=4):
    """Return SIZE*SIZE RGBA rows, with premultiplication undone for storage."""
    boxes = bar_boxes()
    step = 1.0 / supersample
    total = supersample * supersample
    px = bytearray()
    for y in range(SIZE):
        row_start = len(px)
        for x in range(SIZE):
            cov_bg = 0
            cov_bars = [0] * len(BARS)
            for sy in range(supersample):
                fy = y + (sy + 0.5) * step
                for sx in range(supersample):
                    fx = x + (sx + 0.5) * step
                    if rounded_rect_contains(fx, fy, 0, 0, SIZE, SIZE, CORNER):
                        cov_bg += 1
                    for i, (bx, by, bw) in enumerate(boxes):
                        if rounded_rect_contains(
                            fx, fy, bx, by, bw, BAR_H, BAR_R
                        ):
                            cov_bars[i] += 1
            acc = [0, 0, 0, 0]
            if cov_bg:
                acc = list(over((0, 0, 0, 0, 0.0), BG, cov_bg / total))
            for i, (_w, colour) in enumerate(BARS):
                if cov_bars[i]:
                    acc = list(over(tuple(acc), colour, cov_bars[i] / total))
            px.extend(acc[:4])
        # PNG scanlines are prefixed with a filter byte; 0 means "None".
        px.insert(row_start, 0)
    return bytes(px)


def write_png(path, raw, size):
    def chunk(tag, data):
        body = tag + data
        return (
            struct.pack(">I", len(data))
            + body
            + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)
        )

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    png = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )
    path.write_bytes(png)
    return len(png)


# --- svg ---------------------------------------------------------------------

SVG_TEMPLATE = """<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {size} {size}" \
width="{size}" height="{size}" role="img" aria-label="Strata">
  <title>Strata</title>
  <!--
    Source of truth for logo.png. Regenerate both with:
        python assets/generate-logo.py
    Three stacked bars of decreasing width: strata, and the senior/junior split.
    The brightest bar sits on top, because the senior tranche is paid first and
    capped at its target.
  -->
  <rect width="{size}" height="{size}" rx="{corner}" fill="#{bg}"/>
{bars}</svg>
"""


def svg_text():
    bars = []
    for (w, colour), (x, y, _bw) in zip(BARS, bar_boxes()):
        bars.append(
            '  <rect x="%g" y="%g" width="%g" height="%g" rx="%g" fill="#%s"/>'
            % (x, y, w, BAR_H, BAR_R, "".join("%02X" % c for c in colour[:3]))
        )
    return SVG_TEMPLATE.format(
        size=SIZE,
        corner=CORNER,
        bg="".join("%02X" % c for c in BG[:3]),
        bars="\n".join(bars) + "\n",
    )


def main():
    out = Path(__file__).resolve().parent
    (out / "logo.svg").write_text(svg_text(), encoding="utf-8", newline="\n")
    raw = render_rgba()
    n = write_png(out / "logo.png", raw, SIZE)
    print("wrote assets/logo.svg (%d bytes)" % (out / "logo.svg").stat().st_size)
    print("wrote assets/logo.png (%d bytes, %dx%d RGBA)" % (n, SIZE, SIZE))


if __name__ == "__main__":
    main()