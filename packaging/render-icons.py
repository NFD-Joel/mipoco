#!/usr/bin/env python3
"""Render the mipoco app icon into the PNG sizes Linux wants and a Windows .ico.

The vector source of truth is assets/mipoco.svg; this script reproduces the same
design with Pillow (drawn at 4x then downsampled) so no external image tooling or
GI/cairo bindings are required. Run from the repo root:

    python3 packaging/render-icons.py
"""
import os
from PIL import Image, ImageDraw

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
HICOLOR = os.path.join(ROOT, "packaging", "linux", "icons", "hicolor")
ICO = os.path.join(ROOT, "packaging", "windows", "mipoco.ico")
SVG = os.path.join(ROOT, "assets", "mipoco.svg")

PNG_SIZES = [16, 24, 32, 48, 64, 128, 256, 512]
ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]

M = 4  # master canvas is 512*M; everything below is in SVG (512) coordinates

BODY_TOP = (0x25, 0x2c, 0x40)
BODY_BOT = (0x14, 0x18, 0x26)
BAR = (0x2c, 0x34, 0x50)
CYAN = (0x2a, 0xcf, 0xd9)
INACTIVE = (0x4a, 0x54, 0x74)
DIVIDER = (0x3a, 0x43, 0x60)
AMBER = (0xfb, 0xcd, 0x4c)
TEXT1 = (0x5d, 0x68, 0x8a)
TEXT2 = (0x48, 0x50, 0x6e)
STROKE = (0x0a, 0x0d, 0x15)


def s(v):
    return round(v * M)


def vgradient(w, h, top, bot):
    img = Image.new("RGB", (w, h))
    px = img.load()
    for y in range(h):
        t = y / max(h - 1, 1)
        c = tuple(round(top[i] + (bot[i] - top[i]) * t) for i in range(3))
        for x in range(w):
            px[x, y] = c
    return img


def rrect(draw, box, radius, **kw):
    draw.rounded_rectangle([s(box[0]), s(box[1]), s(box[2]), s(box[3])],
                           radius=s(radius), **kw)


def thick_line(draw, pts, width, color):
    w = s(width)
    for (x0, y0), (x1, y1) in zip(pts, pts[1:]):
        draw.line([s(x0), s(y0), s(x1), s(y1)], fill=color, width=w)
    r = w / 2
    for x, y in pts:  # round caps + joins
        draw.ellipse([s(x) - r, s(y) - r, s(x) + r, s(y) + r], fill=color)


def render_master():
    size = 512 * M
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)

    # window body: rounded rect filled with a vertical gradient
    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        [s(40), s(56), s(472), s(456)], radius=s(60), fill=255)
    img.paste(vgradient(size, size, BODY_TOP, BODY_BOT), (0, 0), mask)
    rrect(draw, (40, 56, 472, 456), 60, outline=STROKE, width=s(3))

    # title/tab bar: rounded top corners, flat bottom (clipped to body mask)
    bar = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    bd = ImageDraw.Draw(bar)
    bd.rounded_rectangle([s(40), s(56), s(472), s(144)], radius=s(60), fill=BAR)
    bd.rectangle([s(40), s(104), s(472), s(144)], fill=BAR)
    img.paste(bar, (0, 0), Image.composite(bar.split()[3], Image.new("L", (size, size), 0), mask))

    # tab pills
    rrect(draw, (84, 84, 188, 114), 15, fill=CYAN)
    rrect(draw, (204, 84, 288, 114), 15, fill=INACTIVE)
    rrect(draw, (304, 84, 388, 114), 15, fill=INACTIVE)

    # split divider between the two panes
    thick_line(draw, [(300, 156), (300, 448)], 3, DIVIDER)

    # left pane: prompt chevron + cursor block
    thick_line(draw, [(96, 224), (156, 272), (96, 320)], 15, CYAN)
    rrect(draw, (184, 294, 252, 324), 7, fill=AMBER)

    # right pane: another session's output
    rrect(draw, (332, 206, 436, 222), 8, fill=TEXT1)
    rrect(draw, (332, 248, 416, 264), 8, fill=TEXT2)
    rrect(draw, (332, 290, 432, 306), 8, fill=TEXT2)
    rrect(draw, (332, 332, 404, 348), 8, fill=TEXT2)
    rrect(draw, (332, 374, 424, 390), 8, fill=TEXT2)

    return img


def main():
    master = render_master()
    pngs = {}
    for size in PNG_SIZES:
        out = os.path.join(HICOLOR, f"{size}x{size}", "apps", "mipoco.png")
        os.makedirs(os.path.dirname(out), exist_ok=True)
        master.resize((size, size), Image.LANCZOS).save(out)
        pngs[size] = out
        print("png ", out)

    scalable = os.path.join(HICOLOR, "scalable", "apps", "mipoco.svg")
    os.makedirs(os.path.dirname(scalable), exist_ok=True)
    with open(SVG, "rb") as a, open(scalable, "wb") as b:
        b.write(a.read())
    print("svg ", scalable)

    os.makedirs(os.path.dirname(ICO), exist_ok=True)
    base = Image.open(pngs[max(ICO_SIZES)]).convert("RGBA")
    base.save(ICO, format="ICO", sizes=[(s, s) for s in ICO_SIZES])
    print("ico ", ICO)


if __name__ == "__main__":
    main()
