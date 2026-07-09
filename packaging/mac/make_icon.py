#!/usr/bin/env python3
"""keyboard-it app ikonu üretici (bağımlılık: Pillow).

1024px bir ana logo çizer (gradyan rounded-square + klavye + yönlendirme oku),
tüm .iconset boyutlarını LANCZOS ile küçültür, ardından `iconutil` ile .icns yapar.

Kullanım:
    python3 packaging/mac/make_icon.py
Çıktı:
    crates/mac-sender/assets/keyboard-it.png   (1024 ana logo)
    crates/mac-sender/assets/keyboard-it.icns
"""
import os
import subprocess
import sys
from PIL import Image, ImageDraw

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
ASSETS = os.path.join(ROOT, "crates", "mac-sender", "assets")

S = 1024                      # ana çözünürlük
BG_TOP = (79, 70, 229)        # indigo  #4f46e5
BG_BOT = (6, 182, 212)        # cyan    #06b6d4
KEY = (99, 102, 241)          # klavye tuşları  #6366f1
WHITE = (255, 255, 255, 255)


def lerp(a, b, t):
    return tuple(int(a[i] + (b[i] - a[i]) * t) for i in range(3))


def rounded_mask(size, radius):
    m = Image.new("L", (size, size), 0)
    d = ImageDraw.Draw(m)
    d.rounded_rectangle([0, 0, size - 1, size - 1], radius=radius, fill=255)
    return m


def make_master():
    # Dikey gradyan zemin.
    grad = Image.new("RGB", (S, S), BG_TOP)
    px = grad.load()
    for y in range(S):
        c = lerp(BG_TOP, BG_BOT, y / (S - 1))
        for x in range(S):
            px[x, y] = c

    # Rounded-square (macOS tarzı) maske ile transparan zemine oturt.
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    img.paste(grad, (0, 0), rounded_mask(S, radius=228))

    d = ImageDraw.Draw(img, "RGBA")

    # --- Yönlendirme oku (klavyeden "öteki ekrana" gönderim hissi) ---
    # Sağ-üste doğru kalın beyaz ok; ok başı tek temiz üçgen.
    d.line([(300, 380), (650, 310)], fill=WHITE, width=52, joint="curve")
    d.polygon([(735, 292), (653, 370), (631, 248)], fill=WHITE)

    # --- Klavye gövdesi ---
    bx0, by0, bx1, by1 = 232, 470, 792, 800
    d.rounded_rectangle([bx0, by0, bx1, by1], radius=64, fill=WHITE)

    # Tuş ızgarası (klavye üstünde indigo tuşlar).
    pad = 46
    gap = 26
    cols, rows = 6, 3
    inner_x0 = bx0 + pad
    inner_y0 = by0 + pad
    inner_x1 = bx1 - pad
    inner_y1 = by1 - pad - 70            # en alt sıra "space" için yer
    kw = (inner_x1 - inner_x0 - gap * (cols - 1)) / cols
    kh = (inner_y1 - inner_y0 - gap * (rows - 1)) / rows
    for r in range(rows):
        for c in range(cols):
            x = inner_x0 + c * (kw + gap)
            y = inner_y0 + r * (kh + gap)
            d.rounded_rectangle([x, y, x + kw, y + kh], radius=16, fill=KEY)
    # space bar
    sy0 = inner_y1 + gap
    d.rounded_rectangle([inner_x0 + kw + gap, sy0, inner_x1 - kw - gap, sy0 + 48],
                        radius=16, fill=KEY)

    return img


def main():
    os.makedirs(ASSETS, exist_ok=True)
    master = make_master()
    master_png = os.path.join(ASSETS, "keyboard-it.png")
    master.save(master_png)
    print("yazıldı:", master_png)

    # .iconset klasörü + tüm boyutlar.
    iconset = os.path.join(ASSETS, "keyboard-it.iconset")
    os.makedirs(iconset, exist_ok=True)
    specs = [
        (16, "icon_16x16.png"), (32, "icon_16x16@2x.png"),
        (32, "icon_32x32.png"), (64, "icon_32x32@2x.png"),
        (128, "icon_128x128.png"), (256, "icon_128x128@2x.png"),
        (256, "icon_256x256.png"), (512, "icon_256x256@2x.png"),
        (512, "icon_512x512.png"), (1024, "icon_512x512@2x.png"),
    ]
    for size, name in specs:
        master.resize((size, size), Image.LANCZOS).save(os.path.join(iconset, name))

    icns = os.path.join(ASSETS, "keyboard-it.icns")
    subprocess.run(["iconutil", "-c", "icns", iconset, "-o", icns], check=True)
    print("yazıldı:", icns)


if __name__ == "__main__":
    sys.exit(main())
