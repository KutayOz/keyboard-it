#!/usr/bin/env python3
"""keyboard-it dmg installer-window background generator (dependency: Pillow).

Draws the branded drag-to-Applications background at 1x (660x400) and 2x
(1320x800), then combines both into a retina TIFF with tiffutil. The TIFF is
committed (like the .icns) so CI never needs Pillow; the intermediate PNGs are
gitignored.

Layout contract with packaging/mac/dmg-settings.py (1x coordinates):
    app icon slot           center (180, 205)
    Applications slot       center (480, 205)
    icon size               104  ->  slots span y 153..257; keep them clear

Usage:
    python3 packaging/mac/make_dmg_background.py
Output:
    packaging/mac/dmg-background.png      (660x400, intermediate)
    packaging/mac/dmg-background@2x.png   (1320x800, intermediate)
    packaging/mac/dmg-background.tiff     (retina, committed)
"""
import os
import subprocess
import sys
from PIL import Image, ImageDraw, ImageFont

HERE = os.path.dirname(os.path.abspath(__file__))

# Canvas at 1x; all coordinates below are in this space.
W, H = 660, 400

# Brand palette (matches the app icon: indigo #4f46e5 -> cyan #06b6d4).
BG_TL = (11, 15, 38)          # deep indigo-slate, top-left
BG_BR = (17, 28, 55)          # slightly lighter slate, bottom-right
INDIGO = (79, 70, 229)
CYAN = (6, 182, 212)
WHITE = (255, 255, 255)
MUTED = (148, 163, 184)       # slate-400 for the tagline

# Icon slots (must match dmg-settings.py).
APP_SLOT = (180, 205)
APPS_SLOT = (480, 205)

SS = 2                        # supersampling factor for smooth curves/text


def load_font(size, bold=False):
    """Load a real system font; PIL's bitmap default is never acceptable."""
    # Helvetica.ttc collection indexes: 0 Regular, 1 Bold (verified via getname()).
    want = "Bold" if bold else "Regular"
    for index in range(12):
        try:
            font = ImageFont.truetype("/System/Library/Fonts/Helvetica.ttc",
                                      size, index=index)
        except OSError:
            break
        if font.getname()[1] == want:
            return font
    for path in ("/System/Library/Fonts/SFNS.ttf",
                 "/System/Library/Fonts/SFNSText.ttf"):
        try:
            return ImageFont.truetype(path, size)
        except OSError:
            continue
    raise SystemExit("ERROR: no usable system font (Helvetica.ttc / SFNS.ttf)")


def diagonal_gradient(w, h, c0, c1):
    """Linear gradient along the top-left -> bottom-right diagonal."""
    # Computed on a small grid, then upscaled: smooth and fast.
    gw, gh = 132, 80
    small = Image.new("RGB", (gw, gh))
    px = small.load()
    for y in range(gh):
        for x in range(gw):
            t = (x / (gw - 1) + y / (gh - 1)) / 2
            px[x, y] = tuple(int(c0[i] + (c1[i] - c0[i]) * t) for i in range(3))
    return small.resize((w, h), Image.BICUBIC)


def add_glow(img, center, radius, color, max_alpha):
    """Composite a soft radial glow (quadratic falloff) onto img in place."""
    w, h = img.size
    # Quarter-resolution mask is plenty for a blur-like falloff.
    mw, mh = max(w // 4, 1), max(h // 4, 1)
    mask = Image.new("L", (mw, mh), 0)
    px = mask.load()
    cx, cy = center[0] / 4, center[1] / 4
    r = radius / 4
    for y in range(mh):
        for x in range(mw):
            d = ((x - cx) ** 2 + (y - cy) ** 2) ** 0.5
            if d < r:
                px[x, y] = int(max_alpha * (1 - d / r) ** 2)
    mask = mask.resize((w, h), Image.BICUBIC)
    layer = Image.new("RGB", (w, h), color)
    img.paste(layer, (0, 0), mask)


def draw_keyboard_watermark(img, s):
    """Faint oversized keyboard glyph, cropped by the bottom-right corner."""
    overlay = Image.new("RGBA", img.size, (0, 0, 0, 0))
    d = ImageDraw.Draw(overlay)
    bx0, by0, bx1, by1 = 500 * s, 285 * s, 910 * s, 525 * s   # runs off-canvas
    d.rounded_rectangle([bx0, by0, bx1, by1], radius=44 * s,
                        fill=WHITE + (9,))
    pad, gap = 30 * s, 16 * s
    cols, rows = 6, 3
    ix0, iy0 = bx0 + pad, by0 + pad
    ix1, iy1 = bx1 - pad, by1 - pad - 48 * s   # room for the space row
    kw = (ix1 - ix0 - gap * (cols - 1)) / cols
    kh = (iy1 - iy0 - gap * (rows - 1)) / rows
    for r in range(rows):
        for c in range(cols):
            x = ix0 + c * (kw + gap)
            y = iy0 + r * (kh + gap)
            d.rounded_rectangle([x, y, x + kw, y + kh], radius=10 * s,
                                fill=WHITE + (13,))
    sy = iy1 + gap
    d.rounded_rectangle([ix0 + kw + gap, sy, ix1 - kw - gap, sy + 34 * s],
                        radius=10 * s, fill=WHITE + (13,))
    img.alpha_composite(overlay)


def bezier(p0, c, p1, t):
    """Quadratic bezier point."""
    u = 1 - t
    return (u * u * p0[0] + 2 * u * t * c[0] + t * t * p1[0],
            u * u * p0[1] + 2 * u * t * c[1] + t * t * p1[1])


def draw_arrow(d, s):
    """Smooth white curved arrow between the two icon slots (both left clear)."""
    p0 = (250 * s, 212 * s)          # just right of the app icon slot
    ctrl = (330 * s, 166 * s)        # gentle upward arc
    p1 = (404 * s, 212 * s)          # just left of the Applications slot
    color = WHITE + (232,)
    width = int(5 * s)
    pts = [bezier(p0, ctrl, p1, i / 127) for i in range(128)]

    # Arrowhead: walk back arc-length `hl` from the tip so the head's base sits
    # exactly on the curve, oriented along the local tangent.
    tip = pts[-1]
    hl, hw = 15 * s, 8.5 * s
    acc, i = 0.0, len(pts) - 1
    while i > 0 and acc < hl:
        acc += ((pts[i][0] - pts[i - 1][0]) ** 2 +
                (pts[i][1] - pts[i - 1][1]) ** 2) ** 0.5
        i -= 1
    base = pts[i]
    dx, dy = tip[0] - base[0], tip[1] - base[1]
    n = (dx * dx + dy * dy) ** 0.5
    ux, uy = dx / n, dy / n

    shaft = pts[:i + 1]
    d.line(shaft, fill=color, width=width, joint="curve")
    for end in (shaft[0], shaft[-1]):     # round start cap, seamless head joint
        d.ellipse([end[0] - width / 2, end[1] - width / 2,
                   end[0] + width / 2, end[1] + width / 2], fill=color)
    d.polygon([tip,
               (base[0] - uy * hw, base[1] + ux * hw),
               (base[0] + uy * hw, base[1] - ux * hw)], fill=color)


def render(scale):
    """Render the background at `scale` px per 1x point (supersampled by SS)."""
    s = scale * SS
    w, h = W * s, H * s

    img = diagonal_gradient(w, h, BG_TL, BG_BR).convert("RGBA")
    # Subtle brand glows: indigo upper-left, cyan lower-right. Tasteful, not neon.
    add_glow(img, (90 * s, 10 * s), 320 * s, INDIGO, 46)
    add_glow(img, (580 * s, 400 * s), 400 * s, CYAN, 42)
    draw_keyboard_watermark(img, s)

    d = ImageDraw.Draw(img, "RGBA")

    # Product name + tagline, top-left.
    title_font = load_font(27 * s, bold=True)
    tag_font = load_font(13 * s)
    d.text((32 * s, 28 * s), "keyboard-it", font=title_font,
           fill=WHITE + (247,))
    d.text((33 * s, 68 * s), "drag the app onto Applications to install",
           font=tag_font, fill=MUTED + (255,))

    draw_arrow(d, s)

    # Downsample the supersampled canvas to the target scale.
    return img.resize((W * scale, H * scale), Image.LANCZOS)


def main():
    png_1x = os.path.join(HERE, "dmg-background.png")
    png_2x = os.path.join(HERE, "dmg-background@2x.png")
    tiff = os.path.join(HERE, "dmg-background.tiff")

    render(1).save(png_1x)
    print("wrote:", png_1x)
    render(2).save(png_2x)
    print("wrote:", png_2x)

    # Combine 1x + 2x into a single retina TIFF (Finder picks the right one).
    subprocess.run(["tiffutil", "-cathidpicheck", png_1x, png_2x,
                    "-out", tiff], check=True)
    print("wrote:", tiff)


if __name__ == "__main__":
    sys.exit(main())
