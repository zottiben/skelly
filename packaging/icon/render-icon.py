#!/usr/bin/env python3
"""Render the Skelly app icon (the vertebra brand mark) to a 1024x1024 PNG.

The mark mirrors `skelly_render::logo_chrome_quads` (design guide §02): a vertical
accent spine threading three rounded "diamond" discs - two light `fg.primary` discs
top and bottom, one larger `accent` disc in the middle - centered on a dark rounded
tile in the Ossein Dark brand palette. Drawn at 4x supersampling for clean edges,
then downscaled. This is the *source of truth* for the icon; `build-icns.sh` turns
the PNG into the `.icns` bundled into Skelly.app.
"""

from PIL import Image, ImageDraw

SS = 4  # supersample factor
S = 1024 * SS  # working canvas edge

# Ossein Dark brand tokens (crates/skelly-render/src/theme.rs).
ACCENT = (0xBD, 0x93, 0xF9, 255)  # accent  #BD93F9
LIGHT = (0xCD, 0xD6, 0xF4, 255)  # fg.primary #CDD6F4
BG_TOP = (0x24, 0x22, 0x38)  # a hair lighter, purple-tinted
BG_BOTTOM = (0x14, 0x13, 0x1C)  # near bg.base #181825


def rounded_tile_mask(size, radius):
    """Alpha mask of a rounded square (for the gradient background)."""
    m = Image.new("L", (size, size), 0)
    ImageDraw.Draw(m).rounded_rectangle([0, 0, size - 1, size - 1], radius=radius, fill=255)
    return m


def vertical_gradient(size, top, bottom):
    grad = Image.new("RGB", (1, size))
    px = grad.load()
    for y in range(size):
        t = y / (size - 1)
        px[0, y] = tuple(round(top[i] + (bottom[i] - top[i]) * t) for i in range(3))
    return grad.resize((size, size))


def rounded_diamond(size, radius, color):
    """A rounded square of side `size` rotated 45 deg (a vertebra disc)."""
    pad = max(2, size // 16)
    tile = Image.new("RGBA", (size + 2 * pad, size + 2 * pad), (0, 0, 0, 0))
    ImageDraw.Draw(tile).rounded_rectangle(
        [pad, pad, pad + size, pad + size], radius=radius, fill=color
    )
    return tile.rotate(45, resample=Image.BICUBIC, expand=True)


def paste_centered(base, tile, cx, cy):
    base.alpha_composite(tile, (round(cx - tile.width / 2), round(cy - tile.height / 2)))


def main():
    canvas = Image.new("RGBA", (S, S), (0, 0, 0, 0))

    # Background: purple-tinted vertical gradient clipped to a rounded tile.
    bg = vertical_gradient(S, BG_TOP, BG_BOTTOM).convert("RGBA")
    bg.putalpha(rounded_tile_mask(S, radius=round(0.2246 * S)))  # ~230/1024
    canvas.alpha_composite(bg)

    # The mark, sized to ~0.60 of the canvas and vertically/horizontally centered
    # (mark-space center is at 0.50, 0.50). Fractions match logo_chrome_quads.
    mark = 0.60 * S
    x0 = S / 2 - 0.5 * mark
    y0 = S / 2 - 0.5 * mark
    cx = x0 + 0.5 * mark

    # Spine: an accent rounded pill from 0.09 to 0.91 of the mark height.
    spine_w = 0.06 * mark
    spine = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    ImageDraw.Draw(spine).rounded_rectangle(
        [cx - spine_w / 2, y0 + 0.09 * mark, cx + spine_w / 2, y0 + 0.91 * mark],
        radius=spine_w / 2,
        fill=(*ACCENT[:3], 235),
    )
    canvas.alpha_composite(spine)

    # Three discs threaded on the spine (cy_frac, size_frac, radius_frac, color).
    discs = [
        (0.16, 0.26, 0.26, LIGHT),
        (0.50, 0.42, 0.22, ACCENT),
        (0.84, 0.26, 0.26, LIGHT),
    ]
    for cy_frac, size_frac, radius_frac, color in discs:
        size = round(size_frac * mark)
        d = rounded_diamond(size, round(radius_frac * size), color)
        paste_centered(canvas, d, cx, y0 + cy_frac * mark)

    out = canvas.resize((1024, 1024), Image.LANCZOS)
    out.save("skelly-icon-1024.png")
    print("wrote skelly-icon-1024.png (1024x1024)")


if __name__ == "__main__":
    main()
